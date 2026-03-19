/// Document chunker: splits text into overlapping chunks

/// Split text into chunks of approximately `chunk_size` characters
/// with `overlap` characters of overlap between adjacent chunks.
/// Tries to split on sentence/paragraph boundaries when possible.
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![];
    }

    if text.len() <= chunk_size {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let total = chars.len();
    let mut start = 0;

    while start < total {
        let end = (start + chunk_size).min(total);

        // Try to find a good split point (sentence or paragraph end)
        let split_end = if end < total {
            // Look backwards from end for a good boundary
            let search_from = end.saturating_sub(50);
            let candidate = &chars[search_from..end];

            // Find last sentence boundary
            let sentence_end = candidate
                .iter()
                .enumerate()
                .rev()
                .find(|(_, &c)| matches!(c, '.' | '?' | '!' | '\n'))
                .map(|(i, _)| search_from + i + 1);

            sentence_end.unwrap_or(end)
        } else {
            end
        };

        let chunk: String = chars[start..split_end].iter().collect();
        let chunk = chunk.trim().to_string();
        if !chunk.is_empty() {
            chunks.push(chunk);
        }

        if split_end >= total {
            break;
        }

        // Move start forward, keeping overlap
        start = split_end.saturating_sub(overlap);
        if start >= split_end {
            start = split_end; // Prevent infinite loop
        }
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty() {
        assert!(chunk_text("", 512, 64).is_empty());
    }

    #[test]
    fn test_short() {
        let text = "Hello world";
        let chunks = chunk_text(text, 512, 64);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello world");
    }

    #[test]
    fn test_chunking() {
        let text = "a".repeat(1000);
        let chunks = chunk_text(&text, 200, 20);
        assert!(chunks.len() > 1);
        // All chunks should be <= chunk_size + some tolerance for boundary search
        for chunk in &chunks {
            assert!(chunk.len() <= 250, "Chunk too long: {}", chunk.len());
        }
    }
}
