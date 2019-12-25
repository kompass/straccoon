use crate::{Streamer, StreamerError, Parser, ParserError, ParserErrorKind};
use std::marker::PhantomData;
use ascii::AsciiChar;

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
            },
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


pub struct Byte<S: Streamer, F: FnMut(u8) -> bool>(F, PhantomData<S>);


impl<S: Streamer, F: FnMut(u8) -> bool> Parser for Byte<S, F> {
    type Input = S;
    type Output = u8;

    fn get(&mut self, stream: &mut Self::Input) -> Result<Self::Output, ParserError> {
        match stream.next() {
            Ok(c) => if (self.0)(c) {
                Ok(c)
            } else {
                let unexpected_pos = stream.position();
                stream.before();

                Err(ParserError(unexpected_pos, ParserErrorKind::Unexpected))
            },
            Err(streamer_error) => Err(ParserError(stream.position(), streamer_error.into())),
        }
    }


    fn parse(&mut self, stream: &mut S) -> Result<(), ParserError> {
        self.get(stream).map(|_| ())
    }
}


pub fn byte_predicate<S: Streamer, F: FnMut(u8) -> bool>(predicate: F) -> Byte<S, F> {
    Byte(predicate, PhantomData)
}


macro_rules! ascii_predicate_parser {
    ($name:ident, $f: ident) => {{
        byte_predicate(|c: u8| AsciiChar::from_ascii(c).map(|c| c.$f()).unwrap_or(false))
    }};
    ($name:ident, $f: ident $($args:tt)+) => {{
        byte_predicate(|c: u8| AsciiChar::from_ascii(c).map(|c| c.$f $($args)+).unwrap_or(false))
    }};
}


pub fn alpha_num<S: Streamer>() -> Byte<S, impl FnMut(u8) -> bool> {
    ascii_predicate_parser!(alpha_num, is_alphanumeric)
}


pub fn digit<S: Streamer>() -> Byte<S, impl FnMut(u8) -> bool> {
    ascii_predicate_parser!(digit, is_ascii_digit)
}


pub fn letter<S: Streamer>() -> Byte<S, impl FnMut(u8) -> bool> {
    ascii_predicate_parser!(letter, is_alphabetic)
}


pub fn space<S: Streamer>() -> Byte<S, impl FnMut(u8) -> bool> {
    ascii_predicate_parser!(space, is_ascii_whitespace)
}


pub fn byte<S: Streamer>(b: u8) -> Byte<S, impl FnMut(u8) -> bool> {
    byte_predicate(move |c: u8| c == b)
}


pub struct Bytes<S: Streamer>(&'static [u8], PhantomData<S>);


impl<S: Streamer> Parser for Bytes<S> {
    type Input = S;
    type Output = <Self::Input as Streamer>::Range;

    fn parse(&mut self, stream: &mut S) -> Result<(), ParserError> {
        assert!(self.0.len() > 0);

        for b in self.0 {
            byte_predicate(move |c: u8| c == *b).parse(stream)?;
        }

        Ok(())
    }


    fn get(&mut self, stream: &mut Self::Input) -> Result<Self::Output, ParserError> {
        self.get_range(stream)
    }
}


pub fn bytes<S: Streamer>(range: &'static [u8]) -> Bytes<S> {
    Bytes(range, PhantomData)
}


#[cfg(test)]
mod tests {
    use crate::*;
    use crate::elastic_buffer::ElasticBufferStreamer;
    use super::*;


    #[test]
    fn it_parses_a_precise_byte() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let mut parser = byte(b'T');

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
}
