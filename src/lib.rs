pub mod elastic_buffer;

use ascii::AsciiChar;

pub use elastic_buffer::ElasticBufferStreamer;

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
    // Like reset with a checkpoint one character before, but way faster.
    // The Streamer must guarantee that it is always possible at least once after a next.
    // If this method is called multiple times since the last next, it might panic.
    // TODO : Make a proper documentation
    fn before(&mut self);
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


pub struct Digit;

impl<S: Streamer> Parser<S> for Digit {
    type Output = ();

    fn parse(&mut self, stream: S) -> Result<((), S), ParserError> {
        byte_parser!(digit, is_ascii_digit).parse(stream)
    }
}


pub struct Letter;

impl<S: Streamer> Parser<S> for Letter {
    type Output = ();

    fn parse(&mut self, stream: S) -> Result<((), S), ParserError> {
        byte_parser!(letter, is_alphabetic).parse(stream)
    }
}


pub struct Space;

impl<S: Streamer> Parser<S> for Space {
    type Output = ();

    fn parse(&mut self, stream: S) -> Result<((), S), ParserError> {
        byte_parser!(space, is_ascii_whitespace).parse(stream)
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use super::elastic_buffer::ElasticBufferStreamer;


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
