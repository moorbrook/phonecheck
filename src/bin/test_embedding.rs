/// Test Wav2Vec2 embeddings with captured audio
///
/// Run with: ORT_DYLIB_PATH=/path/to/libonnxruntime.dylib ./target/release/test_embedding
use anyhow::Result;
use phonecheck::embedding::AudioEmbedder;
use std::path::Path;

fn main() -> Result<()> {
    // Load wav file
    let wav_path = Path::new("test_audio.wav");
    if !wav_path.exists() {
        eprintln!("No test_audio.wav found. Run: ./target/release/phonecheck --once --save-audio test_audio.wav");
        std::process::exit(1);
    }

    let mut reader = hound::WavReader::open(wav_path)?;
    let spec = reader.spec();
    println!("WAV spec: {:?}", spec);

    // Read samples and convert to f32
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect();
    println!("Loaded {} samples ({:.2}s)", samples.len(), samples.len() as f32 / spec.sample_rate as f32);

    // Load model
    let model_path = Path::new("models/wav2vec2_encoder.onnx");
    if !model_path.exists() {
        eprintln!("No ONNX model found. Run: uv run --python 3.13 scripts/export_wav2vec2.py");
        std::process::exit(1);
    }

    println!("\nLoading Wav2Vec2 model...");
    let mut embedder = AudioEmbedder::new(model_path)?;

    // Compute embedding for full audio
    println!("Computing full audio embedding...");
    let full_embedding = embedder.embed(&samples)?;
    println!("  Dimension: {}", full_embedding.len());
    println!("  L2 norm: {:.4} (should be ~1.0)",
        full_embedding.iter().map(|x| x * x).sum::<f32>().sqrt());

    // Test with different time segments
    println!("\n=== Segment Similarity Tests ===");

    // First 1 second vs full
    let seg1 = &samples[..16000.min(samples.len())];
    let emb1 = embedder.embed(seg1)?;
    println!("First 1s vs full: {:.4}", AudioEmbedder::cosine_similarity(&emb1, &full_embedding));

    // First 2 seconds vs full
    let seg2 = &samples[..32000.min(samples.len())];
    let emb2 = embedder.embed(seg2)?;
    println!("First 2s vs full: {:.4}", AudioEmbedder::cosine_similarity(&emb2, &full_embedding));

    // Middle 1 second vs full
    let mid_start = samples.len() / 2 - 8000;
    let seg_mid = &samples[mid_start..mid_start + 16000];
    let emb_mid = embedder.embed(seg_mid)?;
    println!("Middle 1s vs full: {:.4}", AudioEmbedder::cosine_similarity(&emb_mid, &full_embedding));

    // Segment similarity matrix
    println!("\n=== Segment Cross-Similarity ===");
    println!("           First1s  First2s  Middle1s  Full");
    println!("First 1s:  {:.4}   {:.4}   {:.4}    {:.4}",
        AudioEmbedder::cosine_similarity(&emb1, &emb1),
        AudioEmbedder::cosine_similarity(&emb1, &emb2),
        AudioEmbedder::cosine_similarity(&emb1, &emb_mid),
        AudioEmbedder::cosine_similarity(&emb1, &full_embedding));
    println!("First 2s:  {:.4}   {:.4}   {:.4}    {:.4}",
        AudioEmbedder::cosine_similarity(&emb2, &emb1),
        AudioEmbedder::cosine_similarity(&emb2, &emb2),
        AudioEmbedder::cosine_similarity(&emb2, &emb_mid),
        AudioEmbedder::cosine_similarity(&emb2, &full_embedding));
    println!("Middle1s:  {:.4}   {:.4}   {:.4}    {:.4}",
        AudioEmbedder::cosine_similarity(&emb_mid, &emb1),
        AudioEmbedder::cosine_similarity(&emb_mid, &emb2),
        AudioEmbedder::cosine_similarity(&emb_mid, &emb_mid),
        AudioEmbedder::cosine_similarity(&emb_mid, &full_embedding));

    println!("\n✓ Wav2Vec2 embeddings working!");
    println!("\nNote: Similar segments of the same recording should have similarity > 0.7");
    println!("Semantically similar phrases ('thanks' vs 'thank you') should also show high similarity.");

    Ok(())
}
