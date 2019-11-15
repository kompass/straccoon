pub mod elastic_buffer;

use elastic_buffer::ElasticBufferStreamer;

#[derive(Debug, Clone, PartialEq)]
pub enum StreamError {
    EndOfInput,
    Other,
}


impl std::convert::From<std::io::Error> for StreamError {
    fn from(stdio_error: std::io::Error) -> StreamError {
        match stdio_error.kind() {
            std::io::ErrorKind::UnexpectedEof => StreamError::EndOfInput,
            _ => StreamError::Other,
        }
    }
}


pub trait Streamer {
    type CheckPoint;

    fn next(&mut self) -> Result<u8, StreamError>;
    fn position(&self) -> u64;
    fn checkpoint(&self) -> Self::CheckPoint;
    fn reset(&mut self, checkpoint: Self::CheckPoint);
}


#[cfg(test)]
mod tests {
    use super::*;
    use super::elastic_buffer::ElasticBufferStreamer;
    use super::elastic_buffer::CHUNK_SIZE;

    #[test]
    fn it_next_on_one_chunk() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = ElasticBufferStreamer::new(fake_read);
        assert_eq!(stream.next(), Ok(b'T'));
        assert_eq!(stream.next(), Ok(b'h'));
        assert_eq!(stream.next(), Ok(b'i'));
        assert_eq!(stream.next(), Ok(b's'));
        assert_eq!(stream.next(), Ok(b' '));

        for _ in 0..12 {
            assert!(stream.next().is_ok());
        }

        assert_eq!(stream.next(), Ok(b'!'));
        assert_eq!(
            stream.next(),
            Err(StreamError::EndOfInput)
        );
    }

    #[test]
    fn it_next_on_multiple_chunks() {
        let mut fake_read = String::with_capacity(CHUNK_SIZE * 3);

        let beautiful_sentence = "This is a sentence, what a beautiful sentence !";
        let number_of_sentences = CHUNK_SIZE * 3 / beautiful_sentence.len();
        for _ in 0..number_of_sentences {
            fake_read += beautiful_sentence;
        }

        let mut stream = ElasticBufferStreamer::new(fake_read.as_bytes());

        assert_eq!(stream.next(), Ok(b'T'));
        assert_eq!(stream.next(), Ok(b'h'));
        assert_eq!(stream.next(), Ok(b'i'));
        assert_eq!(stream.next(), Ok(b's'));
        assert_eq!(stream.next(), Ok(b' '));

        let first_sentence_of_next_chunk_dist =
            CHUNK_SIZE + beautiful_sentence.len() - CHUNK_SIZE % beautiful_sentence.len();
        for _ in 0..first_sentence_of_next_chunk_dist {
            assert!(stream.next().is_ok());
        }

        assert_eq!(stream.next(), Ok(b'i'));
        assert_eq!(stream.next(), Ok(b's'));
        assert_eq!(stream.next(), Ok(b' '));

        for _ in 0..first_sentence_of_next_chunk_dist {
            assert!(stream.next().is_ok());
        }

        assert_eq!(stream.next(), Ok(b'a'));
        assert_eq!(stream.next(), Ok(b' '));

        let dist_to_last_char = number_of_sentences * beautiful_sentence.len()
            - 10 // Letters already read : "This is a "
            - 2 * first_sentence_of_next_chunk_dist
            - 1;
        for _ in 0..dist_to_last_char {
            assert!(stream.next().is_ok());
        }

        assert_eq!(stream.next(), Ok(b'!'));
        assert_eq!(
            stream.next(),
            Err(StreamError::EndOfInput)
        );
    }

    #[test]
    fn it_resets_on_checkpoint() {
        let mut fake_read = String::with_capacity(CHUNK_SIZE * 3);

        let beautiful_sentence = "This is a sentence, what a beautiful sentence !";
        let number_of_sentences = CHUNK_SIZE * 3 / beautiful_sentence.len();
        for _ in 0..number_of_sentences {
            fake_read += beautiful_sentence;
        }

        let mut stream = ElasticBufferStreamer::new(fake_read.as_bytes());

        let first_sentence_of_next_chunk_dist =
            CHUNK_SIZE + beautiful_sentence.len() - CHUNK_SIZE % beautiful_sentence.len();
        for _ in 0..first_sentence_of_next_chunk_dist {
            assert!(stream.next().is_ok());
        }

        assert_eq!(stream.next(), Ok(b'T'));
        assert_eq!(stream.next(), Ok(b'h'));
        assert_eq!(stream.next(), Ok(b'i'));
        assert_eq!(stream.next(), Ok(b's'));
        assert_eq!(stream.next(), Ok(b' '));

        let cp = stream.checkpoint();

        assert_eq!(stream.next(), Ok(b'i'));
        assert_eq!(stream.next(), Ok(b's'));
        assert_eq!(stream.next(), Ok(b' '));

        stream.reset(cp);
        let cp = stream.checkpoint();

        assert_eq!(stream.next(), Ok(b'i'));
        assert_eq!(stream.next(), Ok(b's'));
        assert_eq!(stream.next(), Ok(b' '));

        for _ in 0..first_sentence_of_next_chunk_dist {
            assert!(stream.next().is_ok());
        }

        assert_eq!(stream.next(), Ok(b'a'));

        stream.reset(cp);

        assert_eq!(stream.next(), Ok(b'i'));
        assert_eq!(stream.next(), Ok(b's'));
        assert_eq!(stream.next(), Ok(b' '));
    }

    #[test]
    fn it_free_useless_memory_when_reading_new_chunk() {
        let mut fake_read = String::with_capacity(CHUNK_SIZE * 3);

        let beautiful_sentence = "This is a sentence, what a beautiful sentence !";
        let number_of_sentences = CHUNK_SIZE * 3 / beautiful_sentence.len();
        for _ in 0..number_of_sentences {
            fake_read += beautiful_sentence;
        }

        let mut stream = ElasticBufferStreamer::new(fake_read.as_bytes());

        let cp = stream.checkpoint();
        assert_eq!(stream.buffer_len(), 0);
        assert_eq!(stream.next(), Ok(b'T'));
        assert_eq!(stream.buffer_len(), 1);

        for _ in 0..CHUNK_SIZE {
            assert!(stream.next().is_ok());
        }

        assert_eq!(stream.buffer_len(), 2);

        stream.reset(cp);

        assert_eq!(stream.next(), Ok(b'T'));

        for _ in 0..2 * CHUNK_SIZE {
            assert!(stream.next().is_ok());
        }

        assert_eq!(stream.buffer_len(), 1);
    }
}
