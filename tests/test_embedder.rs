#[cfg(test)]
mod test {
    use anyhow::Result;
    use glowrs::embedding::models::JinaBertBaseV2;
    use glowrs::embedding::sentence_transformer::SentenceTransformer;
    use std::time::Instant;

    #[test]
    fn test_embedder() -> Result<()> {
        let start = Instant::now();

        let embedder: SentenceTransformer<JinaBertBaseV2> = SentenceTransformer::try_new()?;

        let sentences = vec![
            "The cat sits outside",
            "A man is playing guitar",
            "I love pasta",
            "The new movie is awesome",
            "The cat plays in the garden",
            "A woman watches TV",
            "The new movie is so great",
            "Do you like pizza?",
        ];

        let model_load_duration = Instant::now() - start;
        dbg!(format!(
            "Model loaded in {}ms",
            model_load_duration.as_millis()
        ));

        let embeddings = embedder.encode_batch(sentences, true)?;

        dbg!(format!("Pooled embeddings {:?}", embeddings.shape()));
        dbg!(format!(
            "Inference done in {}ms",
            (Instant::now() - start - model_load_duration).as_millis()
        ));

        Ok(())
    }
}
