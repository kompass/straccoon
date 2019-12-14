pub mod elastic_buffer;

use ascii::AsciiChar;
use std::marker::PhantomData;

pub use elastic_buffer::ElasticBufferStreamer;

#[derive(Debug, Clone, PartialEq)]
pub enum StreamError {
    EndOfInput,
    BufferFull,
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
    fn range_from_to_checkpoint(&mut self, from_cp: Self::CheckPoint, to_cp: Self::CheckPoint) -> &[u8];
}


#[derive(Debug, PartialEq)]
pub enum ParserErrorKind {
    Unexpected,
    UnexpectedEndOfInput,
    TooMany,
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

    fn get_between<'a, P: Parser<Input=Self::Input>>(&mut self, stream: &'a mut Self::Input, mut between: P) -> Result<&'a[u8], ParserError>
    {
        between.parse(stream)?;

        let from_cp = stream.checkpoint();

        self.parse(stream)?;

        let to_cp = stream.checkpoint();

        between.parse(stream)?;

        Ok(stream.range_from_to_checkpoint(from_cp, to_cp))
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
                let unexpected_pos = stream.position();
                stream.before();

                Err(ParserError::Lazy(unexpected_pos, ParserErrorKind::Unexpected))
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


pub fn alpha_num<S: Streamer>() -> Satisfy<S, impl FnMut(u8) -> bool> {
    byte_parser!(alpha_num, is_alphanumeric)
}


pub fn digit<S: Streamer>() -> Satisfy<S, impl FnMut(u8) -> bool> {
    byte_parser!(digit, is_ascii_digit)
}


pub fn letter<S: Streamer>() -> Satisfy<S, impl FnMut(u8) -> bool> {
    byte_parser!(letter, is_alphabetic)
}


pub fn space<S: Streamer>() -> Satisfy<S, impl FnMut(u8) -> bool> {
    byte_parser!(space, is_ascii_whitespace)
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
                    if stream.position() > position_watchdog {
                        panic!("Many: the parser wasn't LL1");
                    }

                    break;
                },

                e @ Err(_) => return e,
            }
        }

        Ok(())
    }
}


pub fn many<S: Streamer, P: Parser<Input=S>>(parser: P) -> Many<P> {
    Many(parser)
}


pub struct ManyMax<P>(P, usize);

impl<S: Streamer, P: Parser<Input=S>> Parser for ManyMax<P> {
    type Input = S;

    fn parse(&mut self, stream: &mut S) -> Result<(), ParserError> {
        for _ in 0..self.1 {
            let position_watchdog = stream.position();

            match self.0.parse(stream) {
                Ok(_) => continue,

                Err(ParserError::Lazy(_, ParserErrorKind::Unexpected)) | Err(ParserError::Lazy(_, ParserErrorKind::UnexpectedEndOfInput))=> {
                    if stream.position() > position_watchdog {
                        panic!("ManyMax: the parser wasn't LL1");
                    }

                    return Ok(());
                },

                e @ Err(_) => return e,
            }
        }

        Err(ParserError::Lazy(stream.position(), ParserErrorKind::TooMany))
    }
}


pub fn many_max<P>(parser: P, max_count: usize) -> ManyMax<P> {
    ManyMax(parser, max_count)
}


pub struct Attempt<P>(P);

impl<S: Streamer, P: Parser<Input=S>> Parser for Attempt<P> {
    type Input = S;

    fn parse(&mut self, stream: &mut Self::Input) -> Result<(), ParserError> {
        let cp = stream.checkpoint();

        let parse_status = self.0.parse(stream);

        match parse_status {
            Err(ParserError::Lazy(_, ref kind)) if kind == &ParserErrorKind::Unexpected || kind == &ParserErrorKind::UnexpectedEndOfInput => {
                stream.reset(cp);
            },
            _ => ()
        }

        parse_status
    }
}


pub fn attempt<S: Streamer, P: Parser<Input=S>>(parser: P) -> Attempt<P> {
    Attempt(parser)
}

pub struct NotFollowedBy<P>(P);

// TODO: Implement not_followed_by for parsers reading only one byte (using another trait)
impl<S: Streamer, P: Parser<Input=S>> Parser for NotFollowedBy<P> {
    type Input = S;

    fn parse(&mut self, stream: &mut Self::Input) -> Result<(), ParserError> {
        loop {
            let position_watchdog = stream.position();

            match self.0.parse(stream) {
                Ok(()) => {
                    if stream.position() > position_watchdog + 1 {
                        panic!("NotFollowedBy: the parser parsed more than one byte");
                    }

                    stream.before();

                    return Ok(());
                },
                Err(ParserError::Lazy(_, ref kind)) if kind == &ParserErrorKind::Unexpected || kind == &ParserErrorKind::UnexpectedEndOfInput => {
                    if stream.position() > position_watchdog {
                        panic!("NotFollowedBy: the parser wasn't LL1");
                    }

                    stream.next().unwrap();

                    continue;
                },
                e @ _ => return e,
            }
        }
    }
}

pub fn not_followed_by<S: Streamer, P: Parser<Input=S>>(parser: P) -> NotFollowedBy<P> {
    NotFollowedBy(parser)
}


macro_rules! tuple_parser {
    ($fid: ident $(, $id: ident)*) => {
        #[allow(non_snake_case)]
        #[allow(unused_assignments)]
        #[allow(unused_mut)]
        impl <$fid, $($id),*> Parser for ($fid, $($id),*)
            where $fid: Parser,
                $($id: Parser<Input=$fid::Input>),*
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


pub struct Maybe<P>(P);

impl<S: Streamer, P: Parser<Input=S>> Parser for Maybe<P> {
    type Input = S;

    fn parse(&mut self, stream: &mut Self::Input) -> Result<(), ParserError> {
        let position_watchdog = stream.position();

        match self.0.parse(stream) {
            Err(ParserError::Lazy(_, kind)) if kind == ParserErrorKind::Unexpected || kind == ParserErrorKind::UnexpectedEndOfInput => {
                if stream.position() > position_watchdog {
                    panic!("Maybe: the parser wasn't LL1");
                }

                Ok(())
            },

            e @ _ => e,
        }
    }
}


pub fn maybe<P: Parser>(parser: P) -> Maybe<P> {
    Maybe(parser)
}


pub trait ChoiceParser {
    type Input: Streamer;

    fn choice_parse(&mut self, stream: &mut Self::Input) -> Result<(), ParserError>;
}


macro_rules! choice_parser {
    ($fid: ident $(, $id: ident)*) => {
        #[allow(non_snake_case)]
        #[allow(unused_assignments)]
        #[allow(unused_mut)]
        impl <$fid, $($id),*> ChoiceParser for ($fid, $($id),*)
        where $fid: Parser,
        $($id: Parser<Input=$fid::Input>),*
        {
            type Input = $fid::Input;

            fn choice_parse(&mut self, stream: &mut Self::Input) -> Result<(), ParserError> {
                let (ref mut $fid, $(ref mut $id),*) = *self;

                let position_watchdog = stream.position();

                let mut last = match $fid.parse(stream) {
                    e @ Err(ParserError::Lazy(_, ParserErrorKind::Unexpected)) | e @ Err(ParserError::Lazy(_, ParserErrorKind::UnexpectedEndOfInput)) => {
                        if stream.position() > position_watchdog {
                            panic!("Choice: the parser wasn't LL1");
                        }

                        e
                    },
                    e @ _ => return e,
                };


                $(
                    last = match $id.parse(stream) {
                        e @ Err(ParserError::Lazy(_, ParserErrorKind::Unexpected)) | e @ Err(ParserError::Lazy(_, ParserErrorKind::UnexpectedEndOfInput)) => {
                            if stream.position() > position_watchdog {
                                panic!("Choice: the parser wasn't LL1");
                            }

                            e
                        },
                        e @ _ => return e,
                    };

                )*

                last
            }
        }
    }
}


choice_parser!(A);
choice_parser!(A, B);
choice_parser!(A, B, C);
choice_parser!(A, B, C, D, E);
choice_parser!(A, B, C, D, E, F);
choice_parser!(A, B, C, D, E, F, G);
choice_parser!(A, B, C, D, E, F, G, H);
choice_parser!(A, B, C, D, E, F, G, H, I);
choice_parser!(A, B, C, D, E, F, G, H, I, J);
choice_parser!(A, B, C, D, E, F, G, H, I, J, K);
choice_parser!(A, B, C, D, E, F, G, H, I, J, K, L);
choice_parser!(A, B, C, D, E, F, G, H, I, J, K, L, M);
choice_parser!(A, B, C, D, E, F, G, H, I, J, K, L, M, N);


pub struct Choice<C>(C);

impl<S: Streamer, C: ChoiceParser<Input=S>> Parser for Choice<C> {
    type Input = S;

    fn parse(&mut self, stream: &mut Self::Input) -> Result<(), ParserError> {
        self.0.choice_parse(stream)
    }
}


pub fn choice<C: ChoiceParser>(parsers: C) -> Choice<C> {
    Choice(parsers)
}


#[cfg(test)]
mod tests {
    use super::*;
    use super::elastic_buffer::ElasticBufferStreamer;


    #[test]
    fn it_parses_alpha_num() {
        let fake_read1 = &b"!This is the text !"[..];
        let mut stream1 = ElasticBufferStreamer::unlimited(fake_read1);

        let mut parser1 = alpha_num();
        assert!(parser1.parse(&mut stream1).is_err());

        let fake_read2 = &b"This is the text !"[..];
        let mut stream2 = ElasticBufferStreamer::unlimited(fake_read2);

        let mut parser2 = alpha_num();
        assert!(parser2.parse(&mut stream2).is_ok());
    }

    #[test]
    fn it_gets_parsed_range() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let mut parser = many(alpha_num());

        let rg = parser.get(&mut stream).unwrap();

        assert_eq!(rg, &(b"This")[..]);
    }

    #[test]
    fn it_gets_range_between_delimiters() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let mut parser = (letter(), letter());
        let delimiter = letter();

        let rg = parser.get_between(&mut stream, delimiter).unwrap();

        assert_eq!(rg, &(b"hi")[..]);
    }

    #[test]
    fn it_get_parsed_range_with_max_size() {
        let fake_read = &b"Its HUUUUUUUUUUUUUUUUUUUUUUUUUGE"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let mut parser = many_max(alpha_num(), 5);

        let rg1 = parser.get(&mut stream).unwrap();

        assert_eq!(rg1, &(b"Its")[..]);

        stream.next().unwrap();

        assert_eq!(parser.get(&mut stream), Err(ParserError::Lazy(9, ParserErrorKind::TooMany)));
    }

    #[test]
    fn it_merges_parser_sequences() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

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

    #[test]
    fn it_recovers_on_failed_attempts() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let mut parser = many(attempt((alpha_num(), alpha_num(), alpha_num(), alpha_num(), space())));

        let rg = parser.get(&mut stream).unwrap();
        assert_eq!(rg, &(b"This ")[..]);
    }

    #[test]
    fn it_parses_maybe() {
        let mut parser = (maybe(space()), letter());

        let fake_read1 = &b"This"[..];
        let mut stream1 = ElasticBufferStreamer::unlimited(fake_read1);
        let rg1 = parser.get(&mut stream1).unwrap();
        assert_eq!(rg1, &(b"T")[..]);

        let fake_read2 = &b" is the text !"[..];
        let mut stream2 = ElasticBufferStreamer::unlimited(fake_read2);
        let rg2 = parser.get(&mut stream2).unwrap();
        assert_eq!(rg2, &(b" i")[..]);
    }

    #[test]
    fn it_parses_until_something() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let mut parser = not_followed_by(space());

        let rg = parser.get(&mut stream).unwrap();
        assert_eq!(rg, &(b"This")[..]);
    }

    #[test]
    fn it_chooses_the_good_parser() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let initial_position_cp = stream.checkpoint();

        let mut parser1 = choice((letter(),));

        let rg1 = parser1.get(&mut stream).unwrap();
        assert_eq!(rg1, &(b"T")[..]);

        stream.reset(initial_position_cp);
        let initial_position_cp = stream.checkpoint();

        let mut parser2 = choice((space(), letter()));

        let rg2 = parser2.get(&mut stream).unwrap();
        assert_eq!(rg2, &(b"T")[..]);

        stream.reset(initial_position_cp);
        // let initial_position_cp = stream.checkpoint();

        let mut parser3 = choice((space(), space(), letter()));

        let rg3 = parser3.get(&mut stream).unwrap();
        assert_eq!(rg3, &(b"T")[..]);

    }
}
