pub mod elastic_buffer;

use ascii::AsciiChar;

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


pub enum ParserErrorKind {
    Unexpected,
    UnexpectedEndOfInput,
    InputError(StreamError),
}


pub enum ParserErrorInfo {
    Unexpected(u8), // TODO: explicit more the unexpected and expected data type (contains end-of-input)
    Expected(u8),
    Info(String),
    InputError(StreamError), // Other than end-of-input
}


pub enum ParserError {
    Lazy(u64, ParserErrorKind),
    Detailed(u64, Vec<ParserErrorInfo>),
}


pub trait Parser<S: Streamer> {
    type Output;

    fn parse(&mut self, stream: S) -> Result<(Self::Output, S), ParserError>;
}

pub struct Satisfy<F: FnMut(u8) -> bool> {
    predicate: F,
}


impl<S: Streamer, F: FnMut(u8) -> bool> Parser<S> for Satisfy<F> {
    type Output = ();

    fn parse(&mut self, mut stream: S) -> Result<(Self::Output, S), ParserError> {
        match stream.next() {
            Ok(c) => if (self.predicate)(c) {
                Ok(((), stream))
            } else {
                Err(ParserError::Lazy(stream.position(), ParserErrorKind::Unexpected))
            },

            Err(e) => Err(ParserError::Lazy(stream.position(), ParserErrorKind::InputError(e)))
        }
    }
}


pub fn satisfy<F: FnMut(u8) -> bool>(predicate: F) -> Satisfy<F> {
    Satisfy {
        predicate,
    }
}


macro_rules! byte_parser {
    ($name:ident, $f: ident) => {{
        satisfy(|c: u8| AsciiChar::from_ascii(c).map(|c| c.$f()).unwrap_or(false))
    }};
    ($name:ident, $f: ident $($args:tt)+) => {{
        satisfy(|c: u8| AsciiChar::from_ascii(c).map(|c| c.$f $($args)+).unwrap_or(false))
    }};
}


pub struct AlphaNum;


impl<S: Streamer> Parser<S> for AlphaNum {
    type Output = ();

    fn parse(&mut self, stream: S) -> Result<((), S), ParserError> {
        byte_parser!(alpha_num, is_alphanumeric).parse(stream)
    }
}


pub fn alpha_num() -> AlphaNum {
    AlphaNum
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


    #[test]
    fn it_parses_alpha_num() {
        let fake_read1 = &b"!This is the text !"[..];
        let mut stream1 = ElasticBufferStreamer::new(fake_read1);

        let mut parser1 = alpha_num();
        assert!(parser1.parse(stream1).is_err());

        let fake_read2 = &b"This is the text !"[..];
        let mut stream2 = ElasticBufferStreamer::new(fake_read2);

        let mut parser2 = alpha_num();
        assert!(parser2.parse(stream2).is_ok());
    }
}
