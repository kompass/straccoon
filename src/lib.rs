//! This crate contains parser combinators optimized for streaming data.
//! It can also be used to parse a huge amount of data which can't fit on RAM.
//!
//! It is largely inspired by `combine`, another parser combinators crate,
//! but rethought specifically for streaming data.
//!
//! The two main features are streamers and parser combinators.
//! The streamer handles the data source, ensuring that enough data is backed up.
//! The main streamer, `ElasticBufferStreamer`, is based on the highly optimized ringbuffer
//! from the crate `slice-deque`, and adapts its size dynamically following the demand.
//!
//! The parser combinators are functions which can be combined to form a more complex parser.
//! They take their input from a streamer and creates output values on the fly from
//! parsed tokens accessible by reference, copying only when needed.
pub mod elastic_buffer;
pub mod parsers;

pub use elastic_buffer::ElasticBufferStreamer;

#[derive(Debug, Clone, PartialEq)]
pub enum StreamError {
    EndOfInput,
    BufferFull,
    InputError(std::io::ErrorKind),
}


impl std::convert::From<std::io::Error> for StreamError {
    fn from(stdio_error: std::io::Error) -> StreamError {
        match stdio_error.kind() {
            std::io::ErrorKind::UnexpectedEof => StreamError::EndOfInput,
            e @ _ => StreamError::InputError(e),
        }
    }
}


pub trait StreamerRange{
    type Input: Streamer;

    fn to_ref<'a>(self, input: &'a mut Self::Input) -> &'a [u8];
}


pub trait Streamer {
    /// Type which represents a previous position from which one can return or which can mark the beginning or the end of a range
    type CheckPoint;

    /// Type which represents a reference to a range of bytes from the input.
    /// The streamer guarantees that the byte sequence will always be accessible as long as the Range and the Streamer exist. It's up to the streamer to decide how.
    type Range: StreamerRange;

    /// Returns the next byte of the input according to position.
    ///
    /// # Errors
    /// When the input encounters an error, when the buffer is full (even if the streamer can be dynamically sized, it can be decided that it has a maximum size),
    /// or when we encounter end of input.
    fn next(&mut self) -> Result<u8, StreamError>;

    /// Returns the current position in number of bytes since the beginning of the input.
    fn position(&self) -> u64;

    /// Creates a checkpoint at the actual position and returns it. The Streamer guarantees we can return to that position or get a Range from that position
    /// until the checkpoint is dropped.
    fn checkpoint(&self) -> Self::CheckPoint;

    /// Consumes a checkpoint and set the Streamer position to the one referenced by the checkpoint.
    fn reset(&mut self, checkpoint: Self::CheckPoint);

    /// Like reset with a checkpoint one character before, but can be way faster.
    /// The Streamer must guarantee that it is always possible at least once after a next.
    /// If this method is called multiple times since the last next, it might panic.
    fn before(&mut self);

    /// Gets a Range from gived checkpoint to current position.
    ///
    /// # Panics
    /// It panics when the given checkpoint points to a position after the current one's.
    fn range_from_checkpoint(&mut self, cp: Self::CheckPoint) -> Self::Range {
        self.range_from_to_checkpoint(cp, self.checkpoint())
    }

    /// Creates a Range from the first given checkpoint to the second given one.
    ///
    /// # Panics
    /// It panics when the first given checkpoint points to a position after the second one's.
    fn range_from_to_checkpoint(&mut self, from_cp: Self::CheckPoint, to_cp: Self::CheckPoint) -> Self::Range;
}


#[derive(Debug, PartialEq)]
pub enum ParserErrorKind {
    Unexpected,
    UnexpectedEndOfInput,
    TooMany,
    BufferFull,
    InputError(std::io::ErrorKind),
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
    /// The type of the streamer handling the data source.
    type Input: Streamer;

    /// The type of the parsed data returned by `get()`.
    type Output;

    /// Parses the input without returning any output.
    /// After the call, if a sequence is parsed, `Ok(())` is returned and the position of the streamer is just
    /// after the sequence parsed.
    /// If the sequence failed to parse the sequence, an error is returned with the position where the error was encountered,
    /// and the position of the streamer depends on the implementation.
    fn parse(&mut self, stream: &mut Self::Input) -> Result<(), ParserError>;

    /// Parses the input and if it succeeds, returns a Range of the parsed sequence.
    /// If it fails, acts like `parse()`.
    fn get_range(&mut self, stream: &mut Self::Input) -> Result<<Self::Input as Streamer>::Range, ParserError> {
        let cp = stream.checkpoint();

        self.parse(stream)?;

        Ok(stream.range_from_checkpoint(cp))
    }

    fn get_between<'a, P: Parser<Input=Self::Input>>(&mut self, stream: &'a mut Self::Input, mut between: P) -> Result<<Self::Input as Streamer>::Range, ParserError>
    {
        between.parse(stream)?;

        let from_cp = stream.checkpoint();

        self.parse(stream)?;

        let to_cp = stream.checkpoint();

        between.parse(stream)?;

        Ok(stream.range_from_to_checkpoint(from_cp, to_cp))
    }

    /// Parses the input and if it succeeds, returns the parsed value of type `Self::Output`.
    /// If it fails, acts like `parse()`.
    fn get(&mut self, stream: &mut Self::Input) -> Result<Self::Output, ParserError>;
}


pub struct Many<P>(P);

impl<S: Streamer, P: Parser<Input=S>> Parser for Many<P> {
    type Input = S;
    type Output = Vec<P::Output>;

    /// Applies the given parser as many times as possible (maybe zero times).
    /// Returns an error only when the Streamer is encountering one.
    ///
    /// # Warning
    /// This parser doesn't let the Streamer in its initial position when it fails.
    /// However, it fails only when the Streamer is encountering an error.
    /// If you need the streamer to be in its initial position even when your Streamer
    /// encounters an error, you should use `attempt`.
    ///
    /// # Panics
    /// Panics when the underlying parser fails to parse but lets the streamer at another
    /// position than the one before its call.
    fn parse(&mut self, stream: &mut S) -> Result<(), ParserError> {
        loop {
            let position_watchdog = stream.position();

            match self.0.parse(stream) {
                Ok(_) => continue,

                Err(ParserError::Lazy(_, kind)) if kind == ParserErrorKind::Unexpected || kind == ParserErrorKind::UnexpectedEndOfInput => {
                    if stream.position() != position_watchdog {
                        panic!("Many: the underlying parser failed to another position than the one before its call. Maybe you should use `attempt`.");
                    }

                    break;
                },

                e @ Err(_) => return e,
            }
        }

        Ok(())
    }


    /// Applies the given parser as much as possible (maybe zero times).
    /// Returns an error only when the Streamer is encountering one.
    /// Otherwise, it returns a `Vec` containing all the outputs of the parser.
    ///
    /// # Warning
    /// This parser doesn't let the Streamer in its initial position when it fails.
    /// However, it fails only when the Streamer is encountering an error.
    /// If you need the streamer to be in its initial position even when your Streamer
    /// encounters an error, you should use `attempt`.
    ///
    /// # Panics
    /// Panics when the underlying parser fails to parse but lets the streamer at another
    /// position than the one before its call.
    fn get(&mut self, stream: &mut Self::Input) -> Result<Self::Output, ParserError>  {
        let mut outputs = Vec::new();

        loop {
            let position_watchdog = stream.position();

            match self.0.get(stream) {
                Ok(output) => {
                    outputs.push(output);
                },

                Err(ParserError::Lazy(_, kind)) if kind == ParserErrorKind::Unexpected || kind == ParserErrorKind::UnexpectedEndOfInput => {
                    if stream.position() != position_watchdog {
                        panic!("Many: the underlying parser failed to another position than the one before its call. Maybe you should use `attempt`.");
                    }

                    break;
                },

                Err(e @ _) => return Err(e),
            }
        }

        Ok(outputs)
    }
}


/// Returns a combinator applying the given parser as many times as possible (maybe zero times) and returning
/// a `Vec` of the outputs.
pub fn many<S: Streamer, P: Parser<Input=S>>(parser: P) -> Many<P> {
    Many(parser)
}


pub struct ManyMax<P>(P, usize);

impl<S: Streamer, P: Parser<Input=S>> Parser for ManyMax<P> {
    type Input = S;
    type Output = Vec<P::Output>;

    /// Applies the parser as much as possible (maybe zero times) up to max_count times.
    /// Returns an error when the Streamer is encountering one
    /// or when the underlying parser can be applied too many times.
    /// In this last case, the parser error is of kind `ParserErrorKind::TooMany`.
    /// Otherwise, it succeeds returning a `Vec` containing all the outputs of the parser.
    ///
    /// # Warning
    /// This parser doesn't let the Streamer in its initial position when it fails.
    /// However, it fails only when the Streamer is encountering an error.
    /// If you need the streamer to be in its initial position even when your Streamer
    /// encounters an error, you should use `attempt`.
    ///
    /// # Panics
    /// Panics when the underlying parser fails to parse but lets the streamer at another
    /// position than the one before its call.
    fn get(&mut self, stream: &mut S) -> Result<Self::Output, ParserError> {
        let mut outputs = Vec::new();

        for _ in 0..self.1 {
            let position_watchdog = stream.position();

            match self.0.get(stream) {
                Ok(output) => {
                    outputs.push(output);
                },

                Err(ParserError::Lazy(_, ParserErrorKind::Unexpected)) | Err(ParserError::Lazy(_, ParserErrorKind::UnexpectedEndOfInput))=> {
                    if stream.position() != position_watchdog {
                        panic!("ManyMax: the underlying parser failed to another position than the one before its call. Maybe you should use `attempt`.");
                    }

                    return Ok(outputs);
                },

                Err(e @ _) => return Err(e),
            }
        }

        Err(ParserError::Lazy(stream.position(), ParserErrorKind::TooMany))
    }


    /// Applies the parser as much as possible (maybe zero times) up to max_count times.
    /// Returns an error when the Streamer is encountering one
    /// or when the underlying parser can be applied too many times.
    /// In this last case, the parser error is of kind `ParserErrorKind::TooMany`.
    /// Otherwise, it succeeds returning `Ok(())`.
    ///
    /// # Warning
    /// This parser doesn't let the Streamer in its initial position when it fails.
    /// However, it fails only when the Streamer is encountering an error.
    /// If you need the streamer to be in its initial position even when your Streamer
    /// encounters an error, you should use `attempt`.
    ///
    /// # Panics
    /// Panics when the underlying parser fails to parse but lets the streamer at another
    /// position than the one before its call.
    fn parse(&mut self, stream: &mut S) -> Result<(), ParserError> {
        for _ in 0..self.1 {
            let position_watchdog = stream.position();

            match self.0.parse(stream) {
                Ok(_) => continue,

                Err(ParserError::Lazy(_, ParserErrorKind::Unexpected)) | Err(ParserError::Lazy(_, ParserErrorKind::UnexpectedEndOfInput))=> {
                    if stream.position() != position_watchdog {
                        panic!("ManyMax: the underlying parser failed to another position than the one before its call. Maybe you should use `attempt`.");
                    }

                    return Ok(());
                },

                e @ Err(_) => return e,
            }
        }

        Err(ParserError::Lazy(stream.position(), ParserErrorKind::TooMany))
    }
}


/// Returns a combinator applying the given parser as much as possible (maybe zero times) up to `max_count` times and returning
/// a `Vec` of the outputs.
pub fn many_max<P>(parser: P, max_count: usize) -> ManyMax<P> {
    ManyMax(parser, max_count)
}


pub struct Attempt<P>(P);

impl<S: Streamer, P: Parser<Input=S>> Parser for Attempt<P> {
    type Input = S;
    type Output = P::Output;

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


    fn get(&mut self, stream: &mut Self::Input) -> Result<Self::Output, ParserError> {
        let cp = stream.checkpoint();

        let parse_status = self.0.get(stream);

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
            type Output = ($fid::Output, $($id::Output),*);

            /// Parses a sequence of values by calling each parser of the tuple in order.
            fn parse(&mut self, stream: &mut Self::Input) -> Result<(), ParserError> {
                let (ref mut $fid, $(ref mut $id),*) = *self;

                $fid.parse(stream)?;

                $(
                    $id.parse(stream)?;
                )*

                Ok(())
            }


            /// Parses a sequence of values by calling each parser of the tuple in order and if it succeeds,
            /// returns a tuple containing the outputs.
            fn get(&mut self, stream: &mut Self::Input) -> Result<Self::Output, ParserError> {
                let (ref mut $fid, $(ref mut $id),*) = *self;

                Ok(($fid.get(stream)?, $($id.get(stream)?),*))
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
    type Output = Option<P::Output>;

    fn parse(&mut self, stream: &mut Self::Input) -> Result<(), ParserError> {
        let position_watchdog = stream.position();

        match self.0.parse(stream) {
            Err(ParserError::Lazy(_, kind)) if kind == ParserErrorKind::Unexpected || kind == ParserErrorKind::UnexpectedEndOfInput => {
                if stream.position() != position_watchdog {
                    panic!("Maybe: the underlying parser failed to another position than the one before its call. Maybe you should use `attempt`.");
                }

                Ok(())
            },

            e @ _ => e,
        }
    }


    fn get(&mut self, stream: &mut Self::Input) -> Result<Self::Output, ParserError> {
        let position_watchdog = stream.position();

        match self.0.get(stream) {
            Err(ParserError::Lazy(_, kind)) if kind == ParserErrorKind::Unexpected || kind == ParserErrorKind::UnexpectedEndOfInput => {
                if stream.position() != position_watchdog {
                    panic!("Maybe: the underlying parser failed to another position than the one before its call. Maybe you should use `attempt`.");
                }

                Ok(None)
            },

            e @ _ => e.map(|output| Some(output)),
        }
    }
}


pub fn maybe<P: Parser>(parser: P) -> Maybe<P> {
    Maybe(parser)
}


pub trait ChoiceParser {
    type Input: Streamer;
    type Output;

    fn choice_parse(&mut self, stream: &mut Self::Input) -> Result<(), ParserError>;
    fn choice_get(&mut self, stream: &mut Self::Input) -> Result<Self::Output, ParserError>;
}


macro_rules! choice_parser {
    ($fid: ident $(, $id: ident)*) => {
        #[allow(non_snake_case)]
        #[allow(unused_assignments)]
        #[allow(unused_mut)]
        impl <$fid, $($id),*> ChoiceParser for ($fid, $($id),*)
        where $fid: Parser,
        $($id: Parser<Input=$fid::Input, Output=$fid::Output>),*
        {
            type Input = $fid::Input;
            type Output = $fid::Output;

            fn choice_parse(&mut self, stream: &mut Self::Input) -> Result<(), ParserError> {
                let (ref mut $fid, $(ref mut $id),*) = *self;

                let position_watchdog = stream.position();

                let mut last = match $fid.parse(stream) {
                    e @ Err(ParserError::Lazy(_, ParserErrorKind::Unexpected)) | e @ Err(ParserError::Lazy(_, ParserErrorKind::UnexpectedEndOfInput)) => {
                        if stream.position() != position_watchdog {
                            panic!("Choice: the underlying parser failed to another position than the one before its call. Maybe you should use `attempt`.");
                        }

                        e
                    },
                    e @ _ => return e,
                };


                $(
                    last = match $id.parse(stream) {
                        e @ Err(ParserError::Lazy(_, ParserErrorKind::Unexpected)) | e @ Err(ParserError::Lazy(_, ParserErrorKind::UnexpectedEndOfInput)) => {
                            if stream.position() != position_watchdog {
                                panic!("Choice: the underlying parser failed to another position than the one before its call. Maybe you should use `attempt`.");
                            }

                            e
                        },
                        e @ _ => return e,
                    };

                )*

                last
            }


            fn choice_get(&mut self, stream: &mut Self::Input) -> Result<Self::Output, ParserError> {
                let (ref mut $fid, $(ref mut $id),*) = *self;

                let position_watchdog = stream.position();

                let mut last = match $fid.get(stream) {
                    e @ Err(ParserError::Lazy(_, ParserErrorKind::Unexpected)) | e @ Err(ParserError::Lazy(_, ParserErrorKind::UnexpectedEndOfInput)) => {
                        if stream.position() != position_watchdog {
                            panic!("Choice: the underlying parser failed to another position than the one before its call. Maybe you should use `attempt`.");
                        }

                        e
                    },
                    e @ _ => return e,
                };


                $(
                    last = match $id.get(stream) {
                        e @ Err(ParserError::Lazy(_, ParserErrorKind::Unexpected)) | e @ Err(ParserError::Lazy(_, ParserErrorKind::UnexpectedEndOfInput)) => {
                            if stream.position() != position_watchdog {
                                panic!("Choice: the underlying parser failed to another position than the one before its call. Maybe you should use `attempt`.");
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

impl<C: ChoiceParser> Parser for Choice<C> {
    type Input = C::Input;
    type Output = C::Output;

    fn parse(&mut self, stream: &mut Self::Input) -> Result<(), ParserError> {
        self.0.choice_parse(stream)
    }


    fn get(&mut self, stream: &mut Self::Input) -> Result<Self::Output, ParserError> {
        self.0.choice_get(stream)
    }
}


pub fn choice<C: ChoiceParser>(parsers: C) -> Choice<C> {
    Choice(parsers)
}


#[cfg(test)]
mod tests {
    use super::*;
    use super::elastic_buffer::ElasticBufferStreamer;
    use super::parsers::basic::*;


    #[test]
    fn it_gets_parsed_range() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let mut parser = many(alpha_num());

        let rg = parser.get_range(&mut stream).unwrap().to_ref(&mut stream);

        assert_eq!(rg, &(b"This")[..]);
    }


    #[test]
    fn it_gets_a_precise_range() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let mut parser = bytes(&b"This"[..]);

        let rg = parser.get_range(&mut stream).unwrap().to_ref(&mut stream);

        assert_eq!(rg, &(b"This")[..]);
    }


    #[test]
    fn it_get_parsed_range_with_eof() {
        let fake_read = &b"This"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let mut parser = (many(alpha_num()), eof());

        let rg = parser.get_range(&mut stream).unwrap().to_ref(&mut stream);

        assert_eq!(rg, &(b"This")[..]);

    }


    #[test]
    fn it_gets_range_between_delimiters() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let mut parser = (letter(), letter());
        let delimiter = letter();

        let rg = parser.get_between(&mut stream, delimiter).unwrap().to_ref(&mut stream);

        assert_eq!(rg, &(b"hi")[..]);
    }


    #[test]
    fn it_get_parsed_range_with_max_size() {
        let fake_read = &b"Its HUUUUUUUUUUUUUUUUUUUUUUUUUGE"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let mut parser = many_max(alpha_num(), 5);

        let rg1 = parser.get_range(&mut stream).unwrap().to_ref(&mut stream);

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

        let rg = parser1.get_range(&mut stream).unwrap().to_ref(&mut stream);
        assert_eq!(rg, &(b"T")[..]);

        stream.reset(cp_beginning);
        let cp_beginning = stream.checkpoint();

        let rg = parser2.get_range(&mut stream).unwrap().to_ref(&mut stream);
        assert_eq!(rg, &(b"Th")[..]);

        stream.reset(cp_beginning);

        let rg = parser3.get_range(&mut stream).unwrap().to_ref(&mut stream);
        assert_eq!(rg, &(b"Thi")[..]);
    }


    #[test]
    fn it_recovers_on_failed_attempts() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let mut parser = many(attempt((alpha_num(), alpha_num(), alpha_num(), alpha_num(), space())));

        let rg = parser.get_range(&mut stream).unwrap().to_ref(&mut stream);
        assert_eq!(rg, &(b"This ")[..]);
    }


    #[test]
    fn it_parses_maybe() {
        let mut parser = (maybe(space()), letter());

        let fake_read1 = &b"This"[..];
        let mut stream1 = ElasticBufferStreamer::unlimited(fake_read1);
        let rg1 = parser.get_range(&mut stream1).unwrap().to_ref(&mut stream1);
        assert_eq!(rg1, &(b"T")[..]);

        let fake_read2 = &b" is the text !"[..];
        let mut stream2 = ElasticBufferStreamer::unlimited(fake_read2);
        let rg2 = parser.get_range(&mut stream2).unwrap().to_ref(&mut stream2);
        assert_eq!(rg2, &(b" i")[..]);
    }


    #[test]
    fn it_chooses_the_good_parser() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let initial_position_cp = stream.checkpoint();

        let mut parser1 = choice((letter(),));

        let rg1 = parser1.get_range(&mut stream).unwrap().to_ref(&mut stream);
        assert_eq!(rg1, &(b"T")[..]);

        stream.reset(initial_position_cp);
        let initial_position_cp = stream.checkpoint();

        let mut parser2 = choice((space(), letter()));

        let rg2 = parser2.get_range(&mut stream).unwrap().to_ref(&mut stream);
        assert_eq!(rg2, &(b"T")[..]);

        stream.reset(initial_position_cp);
        // let initial_position_cp = stream.checkpoint();

        let mut parser3 = choice((space(), space(), letter()));

        let rg3 = parser3.get_range(&mut stream).unwrap().to_ref(&mut stream);
        assert_eq!(rg3, &(b"T")[..]);

    }
}
