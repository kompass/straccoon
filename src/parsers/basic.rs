use crate::{Parser, ParserError, ParserErrorKind, Streamer, StreamerError};
use std::marker::PhantomData;

pub struct Eof<S: Streamer>(PhantomData<S>);

impl<S: Streamer> Parser for Eof<S> {
    type Input = S;
    type Output = ();

    fn parse(&mut self, stream: &mut S) -> Result<(), ParserError> {
        match stream.next() {
            Err(StreamerError::EndOfInput) => Ok(()),
            Ok(_) => {
                stream.before();

                Err(ParserError(stream.position(), ParserErrorKind::Unexpected))
            }
            Err(streamer_error) => Err(ParserError(stream.position(), streamer_error.into())),
        }
    }

    fn get(&mut self, stream: &mut Self::Input) -> Result<Self::Output, ParserError> {
        self.parse(stream)
    }
}

pub fn eof<S: Streamer>() -> Eof<S> {
    Eof(PhantomData)
}

pub struct CharPred<S: Streamer, F: FnMut(char) -> bool>(F, PhantomData<S>);

impl<S: Streamer, F: FnMut(char) -> bool> Parser for CharPred<S, F> {
    type Input = S;
    type Output = char;

    fn get(&mut self, stream: &mut Self::Input) -> Result<Self::Output, ParserError> {
        match stream.next() {
            Ok(c) => {
                if (self.0)(c) {
                    Ok(c)
                } else {
                    let unexpected_pos = stream.position();
                    stream.before();

                    Err(ParserError(unexpected_pos, ParserErrorKind::Unexpected))
                }
            }
            Err(streamer_error) => Err(ParserError(stream.position(), streamer_error.into())),
        }
    }

    fn parse(&mut self, stream: &mut S) -> Result<(), ParserError> {
        self.get(stream).map(|_| ())
    }
}

pub fn char_predicate<S: Streamer, F: FnMut(char) -> bool>(predicate: F) -> CharPred<S, F> {
    CharPred(predicate, PhantomData)
}

macro_rules! predicate_parser {
    ($name:ident, $f: ident) => {{
        char_predicate(|c: char| c.$f())
    }};
    ($name:ident, $f: ident $($args:tt)+) => {{
        char_predicate(|c: char| c.$f $($args)+)
    }};
}

pub fn alpha_num<S: Streamer>() -> CharPred<S, impl FnMut(char) -> bool> {
    predicate_parser!(alpha_num, is_alphanumeric)
}

pub fn digit<S: Streamer>() -> CharPred<S, impl FnMut(char) -> bool> {
    predicate_parser!(digit, is_ascii_digit)
}

pub fn letter<S: Streamer>() -> CharPred<S, impl FnMut(char) -> bool> {
    predicate_parser!(letter, is_alphabetic)
}

pub fn space<S: Streamer>() -> CharPred<S, impl FnMut(char) -> bool> {
    predicate_parser!(space, is_ascii_whitespace)
}

pub fn char<S: Streamer>(c: char) -> CharPred<S, impl FnMut(char) -> bool> {
    char_predicate(move |d: char| c == d)
}

pub fn one_of<S: Streamer>(chars: Vec<char>) -> CharPred<S, impl FnMut(char) -> bool> { // TODO : use array of any size when it will be possible.
    char_predicate(move |c: char| chars.iter().any(|&b| c == b))
}

pub fn none_of<S: Streamer>(chars: Vec<char>) -> CharPred<S, impl FnMut(char) -> bool> {
    char_predicate(move |c: char| chars.iter().all(|&b| c != b))
}

pub struct Chars<S: Streamer>(&'static str, PhantomData<S>);

impl<S: Streamer> Parser for Chars<S> {
    type Input = S;
    type Output = <Self::Input as Streamer>::Range;

    fn parse(&mut self, stream: &mut S) -> Result<(), ParserError> {
        assert!(self.0.len() > 0);

        for b in self.0.chars() {
            char_predicate(move |c: char| c == b).parse(stream)?;
        }

        Ok(())
    }

    fn get(&mut self, stream: &mut Self::Input) -> Result<Self::Output, ParserError> {
        self.get_range(stream)
    }
}

pub fn chars<S: Streamer>(range: &'static str) -> Chars<S> {
    Chars(range, PhantomData)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elastic_buffer::ElasticBufferStreamer;

    #[test]
    fn it_parses_a_precise_byte() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let mut parser = char('T');

        assert!(parser.parse(&mut stream).is_ok());
        assert_eq!(stream.position(), 1);
    }

    #[test]
    fn it_parses_alpha_num() {
        let fake_read1 = &b"!This is the text !"[..];
        let mut stream1 = ElasticBufferStreamer::unlimited(fake_read1);

        let mut parser1 = alpha_num();
        assert!(parser1.parse(&mut stream1).is_err());
        assert_eq!(stream1.position(), 0);

        let fake_read2 = &b"This is the text !"[..];
        let mut stream2 = ElasticBufferStreamer::unlimited(fake_read2);

        let mut parser2 = alpha_num();
        assert!(parser2.parse(&mut stream2).is_ok());
        assert_eq!(stream2.position(), 1);
    }

    #[test]
    fn it_parses_eof() {
        let fake_read = &b"A"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let mut parser1 = letter();

        parser1.parse(&mut stream).unwrap();

        let mut parser2 = eof();

        assert!(parser2.parse(&mut stream).is_ok());
    }

    #[test]
    fn it_parses_one_of() {
        let fake_read = &b"Bounga"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let mut parser = one_of(vec!['a', 'B', ')']);

        assert!(parser.parse(&mut stream).is_ok());
        assert!(parser.parse(&mut stream).is_err());
    }

    #[test]
    fn it_parses_none_of() {
        let fake_read = &b"Bounga"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let mut parser = none_of(vec!['a', 'o', ')']);

        assert!(parser.parse(&mut stream).is_ok());
        assert!(parser.parse(&mut stream).is_err());
    }
}
