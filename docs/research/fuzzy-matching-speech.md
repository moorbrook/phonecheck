# Fuzzy String Matching for Speech Recognition

## Current Implementation

PhoneCheck uses:
1. Levenshtein distance for word-level similarity
2. Sequential phrase matching with tolerance for extra words
3. Substring fallback matching

## Phonetic Algorithms

### [Soundex](https://en.wikipedia.org/wiki/Soundex)
Converts words to 4-character codes based on sound.

```
Philip  → P410
Phillip → P410  (same code - phonetically similar)
```

**Pros:**
- Simple, fast
- Good for English names

**Cons:**
- Only considers first 4 sounds
- Poor for non-English
- "Gaurav Dwivedi" = "Gaurav Deshmukh" (same code)

### [Metaphone](https://en.wikipedia.org/wiki/Metaphone)
Improved phonetic algorithm that considers entire word.

```
machinery  → MXNR
machinary  → MXNR (common misspelling - same code)
```

**Pros:**
- Considers full word
- Better than Soundex for longer words
- Double Metaphone handles multiple pronunciations

### [Editex](https://en.wikipedia.org/wiki/Editex)
Edit distance with phonetic awareness - substitutions within same phonetic group cost less.

Phonetic groups:
- a, e, i, o, u (vowels)
- b, p (bilabials)
- c, k, q (velars)
- d, t (dentals)
- etc.

## Common ASR Transcription Errors

| Error Type | Example | Solution |
|------------|---------|----------|
| Homophones | "for" → "four" | Phonetic matching |
| Contractions | "we are" → "we're" | Expansion rules |
| Numbers | "one" → "1" | Number normalization |
| Hesitations | "um", "uh" insertions | Filter stopwords |
| Repetitions | "the the" | Deduplication |
| Near-homophones | "cubic" → "cubik" | Levenshtein tolerance |

## Recommendations for PhoneCheck

### Option 1: Add Phonetic Matching (Light)

Use Double Metaphone for word comparison fallback:

```rust
// Pseudocode
fn words_similar(a: &str, b: &str) -> bool {
    // Try exact match first
    if a == b { return true; }

    // Try Levenshtein
    if levenshtein(a, b) <= max_distance(a) { return true; }

    // Try phonetic match
    let metaphone_a = double_metaphone(a);
    let metaphone_b = double_metaphone(b);
    if metaphone_a.primary == metaphone_b.primary { return true; }
    if metaphone_a.secondary == metaphone_b.secondary { return true; }

    false
}
```

### Option 2: Preprocessing Normalization

Before matching, normalize the transcript:
1. Expand contractions: "we're" → "we are"
2. Convert numbers: "1" → "one"
3. Remove filler words: "um", "uh", "like"
4. Handle hyphenation: "cubic-machinery" → "cubic machinery"

```rust
fn normalize_transcript(text: &str) -> String {
    let mut result = text.to_lowercase();

    // Expand contractions
    result = result.replace("we're", "we are");
    result = result.replace("you're", "you are");
    result = result.replace("it's", "it is");
    // ... etc

    // Remove filler words
    let fillers = ["um", "uh", "like", "you know"];
    for filler in fillers {
        result = result.replace(&format!(" {} ", filler), " ");
    }

    // Normalize whitespace
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}
```

### Option 3: Hybrid Approach (Best)

Combine normalization + phonetic + Levenshtein:

1. Normalize both expected phrase and transcript
2. For each expected word, find best match in transcript using:
   - Exact match (best)
   - Phonetic match (good)
   - Levenshtein ≤1 (acceptable)
3. Check words appear in order

## Rust Crates

- [strsim](https://crates.io/crates/strsim) - String similarity metrics
- [fuzzy-matcher](https://crates.io/crates/fuzzy-matcher) - Fuzzy matching
- [rphonetic](https://crates.io/crates/rphonetic) - Phonetic algorithms (Soundex, Metaphone)

## Sources
- [Phonetics Based Fuzzy Matching](https://medium.com/data-science-in-your-pocket/phonetics-based-fuzzy-string-matching-algorithms-8399aea04718)
- [PostgreSQL fuzzystrmatch](https://www.postgresql.org/docs/current/fuzzystrmatch.html)
- [Soundex Phonetic Algorithm](https://tilores.io/soundex-phonetic-algorithm-online-tool)
- [Talisman Phonetics Library](https://yomguithereal.github.io/talisman/phonetics/)
