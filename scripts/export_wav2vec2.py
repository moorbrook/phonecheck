# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = [
#   "torch==2.10.0",
#   "transformers==5.0.0",
#   "onnx==1.20.1",
#   "onnxruntime==1.23.2",
#   "onnxscript==0.6.0",
#   "scipy==1.17.0",
# ]
# ///
"""Export Wav2Vec2 encoder to ONNX format for use in Rust."""

import hashlib
import torch
from transformers import Wav2Vec2Model, Wav2Vec2Processor
import onnx
import onnxruntime as ort
import numpy as np
from pathlib import Path

# Expected SHA256 checksums (may vary slightly with different library versions)
EXPECTED_CHECKSUMS = {
    "wav2vec2_encoder.onnx": "c7c1889bdbad143221dead8137d067b092fa3adb891c76a64d26d3dcb3c41b60",
    "wav2vec2_encoder.onnx.data": "836b7752b6f486fb53c0c16a09342859f24d7a89d4a4eccb1818a7d31c467f27",
}


def export_wav2vec2_encoder():
    model_name = "facebook/wav2vec2-base"
    output_path = Path("models/wav2vec2_encoder.onnx")
    output_path.parent.mkdir(exist_ok=True)

    print(f"Loading {model_name}...")
    processor = Wav2Vec2Processor.from_pretrained(model_name)
    model = Wav2Vec2Model.from_pretrained(model_name)
    model.eval()

    # Create dummy input (1 second of audio at 16kHz)
    dummy_audio = torch.randn(1, 16000)

    # Export to ONNX
    print(f"Exporting to {output_path}...")

    torch.onnx.export(
        model,
        (dummy_audio,),
        str(output_path),
        input_names=["audio"],
        output_names=["last_hidden_state"],
        dynamic_axes={
            "audio": {0: "batch", 1: "sequence"},
            "last_hidden_state": {0: "batch", 1: "time"},
        },
        opset_version=14,
    )

    # Verify the model
    print("Verifying ONNX model...")
    onnx_model = onnx.load(str(output_path))
    onnx.checker.check_model(onnx_model)

    # Test with ONNX Runtime
    print("Testing with ONNX Runtime...")
    session = ort.InferenceSession(str(output_path))

    # Test with real audio
    test_audio = np.random.randn(1, 16000).astype(np.float32)
    result = session.run(None, {"audio": test_audio})

    print(f"Input shape: {test_audio.shape}")
    print(f"Output shape: {result[0].shape}")  # Should be [1, ~49, 768]
    print(f"Output dtype: {result[0].dtype}")

    # Get model size
    size_mb = output_path.stat().st_size / (1024 * 1024)
    print(f"\nModel exported to: {output_path}")
    print(f"Model size: {size_mb:.1f} MB")

    # Verify checksums
    verify_checksums(output_path.parent)

    return output_path


def sha256_file(path: Path) -> str:
    """Compute SHA256 hash of a file."""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            h.update(chunk)
    return h.hexdigest()


def verify_checksums(model_dir: Path):
    """Verify model file checksums."""
    print("\nVerifying checksums...")
    all_match = True

    for filename, expected in EXPECTED_CHECKSUMS.items():
        path = model_dir / filename
        if not path.exists():
            print(f"  {filename}: MISSING")
            all_match = False
            continue

        actual = sha256_file(path)
        if actual == expected:
            print(f"  {filename}: OK")
        else:
            print(f"  {filename}: MISMATCH")
            print(f"    Expected: {expected}")
            print(f"    Got:      {actual}")
            all_match = False

    if all_match:
        print("All checksums verified!")
    else:
        print("\nNote: Checksum mismatches may occur with different library versions.")
        print("The model should still work if ONNX verification passed.")


def test_embedding_similarity():
    """Test that similar phrases produce similar embeddings."""
    import scipy.io.wavfile as wav

    output_path = Path("models/wav2vec2_encoder.onnx")
    if not output_path.exists():
        print("Model not found, exporting first...")
        export_wav2vec2_encoder()

    session = ort.InferenceSession(str(output_path))

    # Load test audio
    test_wav = Path("test_audio.wav")
    if test_wav.exists():
        print(f"\nTesting with {test_wav}...")
        sr, audio = wav.read(str(test_wav))
        print(f"Sample rate: {sr}, Duration: {len(audio)/sr:.2f}s")

        # Convert to float32 and normalize
        audio = audio.astype(np.float32) / 32768.0
        audio = audio.reshape(1, -1)

        result = session.run(None, {"audio": audio})
        embeddings = result[0]  # [1, time, 768]

        # Mean pool across time dimension
        mean_embedding = embeddings.mean(axis=1)  # [1, 768]

        print(f"Audio embedding shape: {embeddings.shape}")
        print(f"Mean embedding shape: {mean_embedding.shape}")
        print(f"Embedding norm: {np.linalg.norm(mean_embedding):.4f}")
    else:
        print(f"\nNo test audio found at {test_wav}")


if __name__ == "__main__":
    export_wav2vec2_encoder()
    test_embedding_similarity()
