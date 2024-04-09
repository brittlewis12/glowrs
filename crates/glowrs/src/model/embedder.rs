use anyhow::{Context, Error, Result};
use candle_core::{DType, Module, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::{
    bert::Config as BertConfig, jina_bert::Config as JinaBertConfig,
};
use hf_hub::api::sync::ApiRepo;
use serde::de::DeserializeOwned;
use tokenizers::Tokenizer;

// Re-exports
pub use candle_transformers::models::{bert::BertModel, jina_bert::BertModel as JinaBertModel};

use crate::model::device::DEVICE;
use crate::model::utils::normalize_l2;
use crate::{Sentences, Usage};

pub trait LoadableModel: Sized {
    type Config: DeserializeOwned;
    fn load_model(vb: VarBuilder, cfg: &Self::Config) -> Result<Box<dyn EmbedderModel>>;

    fn empty_model() -> Result<Box<dyn EmbedderModel>>;
}

pub trait EmbedderModel: Send + Sync {
    fn inner_forward(&self, token_ids: &Tensor) -> Result<Tensor>;
}

impl LoadableModel for BertModel {
    type Config = BertConfig;
    fn load_model(vb: VarBuilder, cfg: &Self::Config) -> Result<Box<dyn EmbedderModel>> {
        Ok(Box::new(Self::load(vb, cfg)?))
    }

    fn empty_model() -> Result<Box<dyn EmbedderModel>> {
        let vb = VarBuilder::zeros(DType::F32, &DEVICE);
        let cfg = Self::Config::default();
        Self::load_model(vb, &cfg)
    }
}

impl LoadableModel for JinaBertModel {
    type Config = JinaBertConfig;
    fn load_model(vb: VarBuilder, cfg: &Self::Config) -> Result<Box<dyn EmbedderModel>> {
        Ok(Box::new(Self::new(vb, cfg)?))
    }

    fn empty_model() -> Result<Box<dyn EmbedderModel>> {
        let vb = VarBuilder::zeros(DType::F32, &DEVICE);
        let cfg = Self::Config::v2_base();
        Self::load_model(vb, &cfg)
    }
}

impl EmbedderModel for BertModel {
    #[inline]
    fn inner_forward(&self, token_ids: &Tensor) -> Result<Tensor> {
        let token_type_ids = token_ids.zeros_like()?;
        Ok(self.forward(token_ids, &token_type_ids)?)
    }
}

impl EmbedderModel for JinaBertModel {
    #[inline]
    fn inner_forward(&self, token_ids: &Tensor) -> Result<Tensor> {
        Ok(self.forward(token_ids)?)
    }
}

pub(crate) fn load_model_and_tokenizer_gen<L>(
    api: ApiRepo,
) -> Result<(Box<dyn EmbedderModel>, Tokenizer)>
where
    L: LoadableModel,
{
    let model_path = api
        .get("model.safetensors")
        .context("Model repository is not available or doesn't contain `model.safetensors`.")?;

    let config_path = api
        .get("config.json")
        .context("Model repository doesn't contain `config.json`.")?;

    let tokenizer_path = api
        .get("tokenizer.json")
        .context("Model repository doesn't contain `tokenizer.json`.")?;

    let tokenizer = Tokenizer::from_file(tokenizer_path).map_err(Error::msg)?;
    let config_str = std::fs::read_to_string(config_path)?;

    let cfg = serde_json::from_str(&config_str)
		.context(
			"Failed to deserialize config.json. Make sure you have the right EmbedderModel implementation."
		)?;

    let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[model_path], DType::F32, &DEVICE)? };

    let model = L::load_model(vb, &cfg).context("Something went wrong while loading the model.")?;

    Ok((model, tokenizer))
}

#[derive(Debug, PartialEq)]
pub enum EmbedderType {
    Bert,
    JinaBert,
}

pub(crate) fn load_model_and_tokenizer(
    api: ApiRepo,
    embedder_type: EmbedderType,
) -> Result<(Box<dyn EmbedderModel>, Tokenizer)> {
    let (model, tokenizer) = match embedder_type {
        EmbedderType::Bert => load_model_and_tokenizer_gen::<BertModel>(api)?,
        EmbedderType::JinaBert => load_model_and_tokenizer_gen::<JinaBertModel>(api)?,
    };
    Ok((model, tokenizer))
}

pub(crate) fn encode_batch_with_usage(
    model: &dyn EmbedderModel,
    tokenizer: &Tokenizer,
    sentences: impl Into<Vec<String>>,
    normalize: bool,
) -> Result<(Tensor, Usage)> {
    let tokens = tokenizer
        .encode_batch(sentences.into(), true)
        .map_err(Error::msg)
        .context("Failed to encode batch.")?;

    let prompt_tokens = tokens.len() as u32;

    let token_ids = tokens
        .iter()
        .map(|tokens| {
            let tokens = tokens.get_ids().to_vec();
            Tensor::new(tokens.as_slice(), &DEVICE)
        })
        .collect::<candle_core::Result<Vec<_>>>()?;

    let token_ids = Tensor::stack(&token_ids, 0)?;

    tracing::trace!("running inference on batch {:?}", token_ids.shape());
    let embeddings = model.inner_forward(&token_ids)?;
    tracing::trace!("generated embeddings {:?}", embeddings.shape());

    // Apply some avg-pooling by taking the mean model value for all tokens (including padding)
    let (_n_sentence, out_tokens, _hidden_size) = embeddings.dims3()?;
    let embeddings = (embeddings.sum(1)? / (out_tokens as f64))?;
    let embeddings = if normalize {
        normalize_l2(&embeddings)?
    } else {
        embeddings
    };

    // TODO: Incorrect usage calculation - fix
    let usage = Usage {
        prompt_tokens,
        total_tokens: prompt_tokens + (out_tokens as u32),
    };
    Ok((embeddings, usage))
}

pub(crate) fn encode_batch(
    model: &dyn EmbedderModel,
    tokenizer: &Tokenizer,
    sentences: Sentences,
    normalize: bool,
) -> Result<Tensor> {
    let (out, _) = encode_batch_with_usage(model, tokenizer, sentences, normalize)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hf_hub::api::sync::Api;
    use hf_hub::{Repo, RepoType};

    // TODO: Move to integration tests
    #[test]
    fn test_load_sentence_transformers() {
        let repo_name = "sentence-transformers/all-MiniLM-L6-v2";
        let revision = "refs/pr/21";
        let api = Api::new().unwrap().repo(Repo::with_revision(
            repo_name.into(),
            RepoType::Model,
            revision.into(),
        ));
        let (_model, _tokenizer) = load_model_and_tokenizer_gen::<BertModel>(api).unwrap();
    }
}
