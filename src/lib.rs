pub mod elastic_buffer;

use ascii::AsciiChar;
use std::marker::PhantomData;

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
    fn range_from_checkpoint(&mut self, cp: Self::CheckPoint) -> &[u8];
}


#[derive(Debug, PartialEq)]
pub enum ParserErrorKind {
    Unexpected,
    UnexpectedEndOfInput,
    InputError(StreamError),
}


#[derive(Debug, PartialEq)]
pub enum ParserErrorInfo {
    Unexpected(u8), // TODO: explicit more the unexpected and expected data type (contains end-of-input)
    Expected(u8),
    Info(String),
    InputError(StreamError), // Other than end-of-input
}


#[derive(Debug, PartialEq)]
pub enum ParserError {
    Lazy(u64, ParserErrorKind),
    Detailed(u64, Vec<ParserErrorInfo>),
}


pub trait Parser {
    type Input: Streamer;

    fn parse(&mut self, stream: &mut Self::Input) -> Result<(), ParserError>;

    fn get<'a>(&mut self, stream: &'a mut Self::Input) -> Result<&'a[u8], ParserError>
    {
        let cp = stream.checkpoint();

        self.parse(stream)?;

        Ok(stream.range_from_checkpoint(cp))
    }
}

pub struct Satisfy<S: Streamer, F: FnMut(u8) -> bool>(F, PhantomData<S>);


impl<S: Streamer, F: FnMut(u8) -> bool> Parser for Satisfy<S, F> {
    type Input = S;

    fn parse(&mut self, stream: &mut S) -> Result<(), ParserError> {
        match stream.next() {
            Ok(c) => if (self.0)(c) {
                Ok(())
            } else {
                Err(ParserError::Lazy(stream.position(), ParserErrorKind::Unexpected))
            },

            Err(e) => Err(ParserError::Lazy(stream.position(), ParserErrorKind::InputError(e)))
        }
    }
}


pub fn satisfy<S: Streamer, F: FnMut(u8) -> bool>(predicate: F) -> Satisfy<S, F> {
    Satisfy(predicate, PhantomData)
}


macro_rules! byte_parser {
    ($name:ident, $f: ident) => {{
        satisfy(|c: u8| AsciiChar::from_ascii(c).map(|c| c.$f()).unwrap_or(false))
    }};
    ($name:ident, $f: ident $($args:tt)+) => {{
        satisfy(|c: u8| AsciiChar::from_ascii(c).map(|c| c.$f $($args)+).unwrap_or(false))
    }};
}


pub struct AlphaNum<S: Streamer>(PhantomData<S>);


impl<S: Streamer> Parser for AlphaNum<S> {
    type Input = S;

    fn parse(&mut self, stream: &mut S) -> Result<(), ParserError> {
        byte_parser!(alpha_num, is_alphanumeric).parse(stream)
    }
}


pub fn alpha_num<S: Streamer>() -> AlphaNum<S> {
    AlphaNum(PhantomData)
}


pub struct Digit<S: Streamer>(PhantomData<S>);

impl<S: Streamer> Parser for Digit<S> {
    type Input = S;

    fn parse(&mut self, stream: &mut S) -> Result<(), ParserError> {
        byte_parser!(digit, is_ascii_digit).parse(stream)
    }
}


pub fn digit<S: Streamer>() -> Digit<S> {
    Digit(PhantomData)
}


pub struct Letter<S: Streamer>(PhantomData<S>);

impl<S: Streamer> Parser for Letter<S> {
    type Input = S;

    fn parse(&mut self, stream: &mut S) -> Result<(), ParserError> {
        byte_parser!(letter, is_alphabetic).parse(stream)
    }
}


pub fn letter<S: Streamer>() -> Letter<S> {
    Letter(PhantomData)
}


pub struct Space<S: Streamer>(PhantomData<S>);

impl<S: Streamer> Parser for Space<S> {
    type Input = S;

    fn parse(&mut self, stream: &mut S) -> Result<(), ParserError> {
        byte_parser!(space, is_ascii_whitespace).parse(stream)
    }
}


pub fn space<S: Streamer>() -> Space<S> {
    Space(PhantomData)
}


pub struct Many<P>(P);

impl<S: Streamer, P: Parser<Input=S>> Parser for Many<P> {
    type Input = S;
    
    fn parse(&mut self, stream: &mut S) -> Result<(), ParserError> {
        loop {
            let position_watchdog = stream.position();

            match self.0.parse(stream) {
                Ok(_) => continue,

                Err(ParserError::Lazy(_, kind)) if kind == ParserErrorKind::Unexpected || kind == ParserErrorKind::UnexpectedEndOfInput => {
                    if stream.position() > position_watchdog + 1 {
                        panic!("Many: the parser wasn't LL1");
                    }

                    if stream.position() == position_watchdog + 1 {
                        stream.before();
                    }

                    break;
                },

                e @ Err(_) => return e,
            }
        }

        Ok(())
    }
}


macro_rules! tuple_parser {
    ($fid: ident $(, $id: ident)*) => {
        #[allow(non_snake_case)]
        #[allow(unused_assignments)]
        #[allow(unused_mut)]
        impl <$fid, $($id),*> Parser for ($fid, $($id),*)
            where $fid: Parser,
                $($id: Parser<Input=$fid::Input>),*
                // $(<$id as Parser>::Input == <$fid as Parser>::Input),*
        {
            type Input = $fid::Input;

            fn parse(&mut self, stream: &mut Self::Input) -> Result<(), ParserError> {
                let (ref mut $fid, $(ref mut $id),*) = *self;

                let mut last = $fid.parse(stream)?;

                $(
                    last = $id.parse(stream)?;
                )*

                Ok(last)
            }
        }
    }
}


tuple_parser!(A);
tuple_parser!(A, B);
tuple_parser!(A, B, C);
tuple_parser!(A, B, C, D);
tuple_parser!(A, B, C, D, E);
tuple_parser!(A, B, C, D, E, F);
tuple_parser!(A, B, C, D, E, F, G);
tuple_parser!(A, B, C, D, E, F, G, H);
tuple_parser!(A, B, C, D, E, F, G, H, I);
tuple_parser!(A, B, C, D, E, F, G, H, I, J);
tuple_parser!(A, B, C, D, E, F, G, H, I, J, K);
tuple_parser!(A, B, C, D, E, F, G, H, I, J, K, L);
tuple_parser!(A, B, C, D, E, F, G, H, I, J, K, L, M);
tuple_parser!(A, B, C, D, E, F, G, H, I, J, K, L, M, N);


pub fn many<S: Streamer, P: Parser<Input=S>>(parser: P) -> Many<P> {
    Many(parser)
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
        assert!(parser1.parse(&mut stream1).is_err());

        let fake_read2 = &b"This is the text !"[..];
        let mut stream2 = ElasticBufferStreamer::new(fake_read2);

        let mut parser2 = alpha_num();
        assert!(parser2.parse(&mut stream2).is_ok());
    }

    #[test]
    fn it_gets_parsed_range() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = ElasticBufferStreamer::new(fake_read);

        let mut parser = many(alpha_num());

        let rg = parser.get(&mut stream).unwrap();

        assert_eq!(rg, &(b"This")[..]);
    }

    #[test]
    fn it_merges_parser_sequences() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = ElasticBufferStreamer::new(fake_read);

        let mut parser1 = (alpha_num(),);
        let mut parser2 = (alpha_num(), alpha_num());
        let mut parser3 = (alpha_num(), alpha_num(), alpha_num());

        let cp_beginning = stream.checkpoint();

        let rg = parser1.get(&mut stream).unwrap();
        assert_eq!(rg, &(b"T")[..]);

        stream.reset(cp_beginning);
        let cp_beginning = stream.checkpoint();

        let rg = parser2.get(&mut stream).unwrap();
        assert_eq!(rg, &(b"Th")[..]);

        stream.reset(cp_beginning);

        let rg = parser3.get(&mut stream).unwrap();
        assert_eq!(rg, &(b"Thi")[..]);
    }
}
