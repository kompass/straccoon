use owning_ref::OwningHandle;
use slice_deque::SliceDeque;
use std::cell::{Cell, RefCell};
use std::io::Read;
use std::marker::PhantomData;
use std::rc::{Rc, Weak};
use std::str::Chars;
use tinyvec::ArrayVec;

use super::Streamer;
use super::StreamerError;
use super::StreamerRange;

const ITEM_INDEX_SIZE: usize = 13;
pub const CHUNK_SIZE: usize = 1 << ITEM_INDEX_SIZE;
pub const INITIAL_CAPACITY: usize = 4;

pub type InternalCheckPoint = Cell<usize>;

#[derive(Clone)]
pub struct CheckPoint(Rc<InternalCheckPoint>);

impl CheckPoint {
    fn new(pos: usize) -> CheckPoint {
        CheckPoint(Rc::new(Cell::new(pos)))
    }

    fn inner(&self) -> usize {
        self.0.get()
    }
}

struct CheckPointHandler(Weak<InternalCheckPoint>);

impl CheckPointHandler {
    fn from_checkpoint(cp: &CheckPoint) -> CheckPointHandler {
        let weak_ref = Rc::downgrade(&cp.0);

        CheckPointHandler(weak_ref)
    }
}

struct CheckPointSet(RefCell<Vec<CheckPointHandler>>);

impl CheckPointSet {
    fn new() -> CheckPointSet {
        CheckPointSet(RefCell::new(Vec::new()))
    }

    fn insert(&self, pos: usize) -> CheckPoint {
        let cp = CheckPoint::new(pos);
        self.0
            .borrow_mut()
            .push(CheckPointHandler::from_checkpoint(&cp));

        cp
    }

    fn min_checkpoint_position(&self) -> Option<usize> {
        let mut min: Option<usize> = None;

        self.0.borrow_mut().retain(|cp| {
            if let Some(intern) = cp.0.upgrade() {
                let pos = intern.get();

                let min_val = min.map_or(pos, |min_val| pos.min(min_val));
                min = Some(min_val);

                true
            } else {
                false
            }
        });

        min
    }

    fn sub_offset(&self, value: usize) {
        for cp in self.0.borrow().iter() {
            let handled = cp.0.upgrade().unwrap(); // sub_offset has to be called right after min_checkpoint_position, so there is no unhandled value
            handled.set(handled.get() - value);
        }
    }
}

pub struct Range<R: Read>(
    CheckPoint,
    CheckPoint,
    PhantomData<ElasticBufferStreamer<R>>,
);

impl<R: Read> StreamerRange for Range<R> {
    type Input = ElasticBufferStreamer<R>;

    fn to_ref<'a>(self, stream: &'a mut ElasticBufferStreamer<R>) -> &'a [u8] {
        (*stream).range_ref_from_to_checkpoint(self.0, self.1)
    }
}

struct CharIterHandler<'a>(Chars<'a>);

impl<'a> std::ops::Deref for CharIterHandler<'a> {
    type Target = Chars<'a>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> std::ops::DerefMut for CharIterHandler<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub struct ElasticBufferStreamer<R: Read> {
    raw_read: R,
    buffer: Option<OwningHandle<Box<SliceDeque<u8>>, CharIterHandler<'static>>>,
    max_size: Option<usize>,
    eof: bool,
    non_complete_char: ArrayVec<[u8; 3]>,
    cursor_pos: usize,
    before_pos: Option<usize>,
    checkpoints: CheckPointSet,
    offset: u64,
}

impl<R: Read> ElasticBufferStreamer<R> {
    fn with_maybe_max_size(read: R, max_size: Option<usize>) -> Self {
        let deque = SliceDeque::with_capacity(INITIAL_CAPACITY);
        Self {
            raw_read: read,
            buffer: Some(OwningHandle::new_with_fn(Box::new(deque), |_buffer| {
                CharIterHandler("".chars())
            })),
            max_size,
            eof: false,
            non_complete_char: ArrayVec::new(),
            cursor_pos: 0,
            before_pos: None,
            checkpoints: CheckPointSet::new(),
            offset: 0,
        }
    }

    pub fn unlimited(read: R) -> Self {
        Self::with_maybe_max_size(read, None)
    }

    pub fn with_max_size(read: R, max_size: usize) -> Self {
        Self::with_maybe_max_size(read, Some(max_size))
    }

    /// Warning : `new_cursor_pos` has to be the begin of a string.
    fn set_cursor_pos(&mut self, new_cursor_pos: usize) {
        self.cursor_pos = new_cursor_pos;
        let buffer = self.buffer.take().unwrap().into_owner();
        self.set_char_iter(buffer);
    }

    /// |      BUFFERED       |         NEWLY READ              |
    /// |-----------------|---|-----------------------------|---|
    ///           |         |                                 |
    ///           |        non complete last buffered char    |
    ///           |        (0-3 bytes)                       non complete newly read char
    ///           |                                          (0-3 bytes)
    ///          valid str (checked at last method call)
    ///
    /// After call :
    ///
    /// |-----------------|---|-----------------------------|---|
    /// |    valid str (checked during this method call)    | |
    ///                                                       |
    ///                                                  non complete last buffered char
    ///                                                  (0-3 bytes)
    fn read_new_chunk_from_raw(&mut self) -> Result<(), StreamerError> {
        let mut buffer = self.buffer.take().unwrap().into_owner();
        let valid_buffered_len = buffer.len();

        buffer.extend_from_slice(self.non_complete_char.as_slice());
        let total_buffered_len = buffer.len();

        buffer.resize(total_buffered_len + CHUNK_SIZE, 0);

        let io_result = self.raw_read.read(&mut buffer[total_buffered_len..]);

        if let Ok(newly_read_len) = io_result {
            buffer.truncate(total_buffered_len + newly_read_len);

            if newly_read_len == 0 {
                buffer.truncate(valid_buffered_len);
                self.eof = true;
            } else {
                // Check if buffer is a valid utf8 str (maybe with an non complete char at the end).
                // `buffer[..valid_buffered_len]` is already checked, so we only check `buffer[valid_buffered_len..]`.
                if let Err(utf8_error) = std::str::from_utf8(&buffer[valid_buffered_len..]) {
                    let invalid_bytes: usize =
                        buffer.len() - valid_buffered_len - utf8_error.valid_up_to();

                    if invalid_bytes <= 3 {
                        self.non_complete_char.clear();
                        self.non_complete_char
                            .extend_from_slice(&buffer[(buffer.len() - invalid_bytes)..]);
                        buffer.truncate(buffer.len() - invalid_bytes);
                    } else {
                        buffer.truncate(valid_buffered_len);
                        return Err(StreamerError::Utf8Error);
                    }
                }
            }
        } else {
            buffer.truncate(valid_buffered_len);
        }

        self.set_char_iter(buffer);

        match io_result {
            Ok(_) => Ok(()),
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {
                self.read_new_chunk_from_raw()
            }
            Err(e) => Err(e.into()),
        }
    }

    fn set_char_iter(&mut self, buffer: Box<SliceDeque<u8>>) {
        self.buffer = Some(OwningHandle::new_with_fn(buffer, |buffer| unsafe {
            let buffer = buffer.as_ref().unwrap();
            CharIterHandler(
                std::str::from_utf8_unchecked(buffer.as_slice())
                    [self.cursor_pos.min(buffer.len())..]
                    .chars(),
            )
        }));
    }

    fn free_useless_data(&mut self) -> usize {
        let checkpoint_pos_min = self.checkpoints.min_checkpoint_position();
        let cursor_pos_min = self.before_pos.unwrap_or(self.cursor_pos);
        let useless_amount =
            checkpoint_pos_min.map_or(cursor_pos_min, |cp_min| cp_min.min(cursor_pos_min));

        let mut buffer = self.buffer.take().unwrap().into_owner();
        buffer.drain(..useless_amount);

        self.cursor_pos -= useless_amount;
        self.before_pos = self
            .before_pos
            .map(|before_pos_val| before_pos_val - useless_amount);
        self.offset += useless_amount as u64;
        self.checkpoints.sub_offset(useless_amount);
        let final_len = buffer.len();
        self.set_char_iter(buffer);

        final_len
    }

    pub fn buffer_len(&self) -> usize {
        self.buffer.as_ref().unwrap().as_owner().len()
    }

    fn range_ref_from_to_checkpoint(&mut self, from_cp: CheckPoint, to_cp: CheckPoint) -> &[u8] {
        let range_begin = from_cp.inner();
        let range_end = to_cp.inner();

        let slice = self.buffer.as_ref().unwrap().as_owner().as_slice();

        &slice[range_begin..range_end]
    }
}

impl<R: Read> Streamer for ElasticBufferStreamer<R> {
    type CheckPoint = CheckPoint;
    type Range = Range<R>;

    fn next(&mut self) -> Result<char, StreamerError> {
        if let Some(c) = (*self.buffer.as_mut().unwrap()).next() {
            self.before_pos = Some(self.cursor_pos);
            self.cursor_pos += c.len_utf8();
            Ok(c)
        } else {
            if !self.eof {
                let len_before_read = self.free_useless_data();
                if let Some(max_size) = self.max_size {
                    if len_before_read + CHUNK_SIZE > max_size {
                        return Err(StreamerError::BufferFull);
                    }
                }

                self.read_new_chunk_from_raw()?;
                self.next()
            } else {
                Err(StreamerError::EndOfInput)
            }
        }
    }

    fn position(&self) -> u64 {
        self.offset + self.cursor_pos as u64
    }

    fn checkpoint(&self) -> CheckPoint {
        self.checkpoints.insert(self.cursor_pos)
    }

    fn reset(&mut self, checkpoint: CheckPoint) {
        self.set_cursor_pos(checkpoint.inner());
    }

    fn before(&mut self) {
        if let Some(before_pos) = self.before_pos {
            self.set_cursor_pos(before_pos);
            self.before_pos = None;
        } else {
            panic!("Multiple `before` calls since the last `next`");
        }
    }

    fn range_from_to_checkpoint(&mut self, from_cp: CheckPoint, to_cp: CheckPoint) -> Range<R> {
        assert!(from_cp.inner() <= to_cp.inner());

        Range(from_cp, to_cp, PhantomData)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UTF8_LOREM_IPSUM : &str = "What a beautiful sentence that 念北受決評念北受決評念北受決評念北受決評念北受決評念北受決評 ! Doe's it make sense ! Probably no.";

    #[test]
    fn it_goes_next_on_one_chunk() {
        let mut stream = ElasticBufferStreamer::unlimited(UTF8_LOREM_IPSUM.as_bytes());
        assert_eq!(stream.next(), Ok('W'));
        assert_eq!(stream.next(), Ok('h'));
        assert_eq!(stream.next(), Ok('a'));
        assert_eq!(stream.next(), Ok('t'));
        assert_eq!(stream.next(), Ok(' '));

        for _ in 0..(UTF8_LOREM_IPSUM.chars().count() - 6) {
            assert!(stream.next().is_ok());
        }

        assert_eq!(stream.next(), Ok('.'));
        assert_eq!(stream.next(), Err(StreamerError::EndOfInput));
    }

    #[test]
    fn it_goes_next_on_multiple_chunks() {
        let mut fake_read = String::with_capacity(CHUNK_SIZE * 3);

        let lorem_ipsum_len = UTF8_LOREM_IPSUM.chars().count();
        let number_of_sentences = CHUNK_SIZE * 3 / lorem_ipsum_len;
        for _ in 0..number_of_sentences {
            fake_read += UTF8_LOREM_IPSUM;
        }

        let mut stream = ElasticBufferStreamer::unlimited(fake_read.as_bytes());

        assert_eq!(stream.next(), Ok('W'));
        assert_eq!(stream.next(), Ok('h'));
        assert_eq!(stream.next(), Ok('a'));
        assert_eq!(stream.next(), Ok('t'));
        assert_eq!(stream.next(), Ok(' '));

        let first_sentence_of_next_chunk_dist =
            CHUNK_SIZE + lorem_ipsum_len - CHUNK_SIZE % lorem_ipsum_len;
        for _ in 0..first_sentence_of_next_chunk_dist {
            assert!(stream.next().is_ok());
        }

        assert_eq!(stream.next(), Ok('a'));
        assert_eq!(stream.next(), Ok(' '));
        assert_eq!(stream.next(), Ok('b'));

        for _ in 0..first_sentence_of_next_chunk_dist {
            assert!(stream.next().is_ok());
        }

        assert_eq!(stream.next(), Ok('e'));
        assert_eq!(stream.next(), Ok('a'));

        let dist_to_last_char = number_of_sentences * lorem_ipsum_len
            - 10 // Letters already read : "This is a "
            - 2 * first_sentence_of_next_chunk_dist
            - 1;
        for _ in 0..dist_to_last_char {
            assert!(stream.next().is_ok());
        }

        assert_eq!(stream.next(), Ok('.'));
        assert_eq!(stream.next(), Err(StreamerError::EndOfInput));
    }

    #[test]
    fn it_resets_on_checkpoint() {
        let mut fake_read = String::with_capacity(CHUNK_SIZE * 3);

        let beautiful_sentence = "This is a sentence, what a beautiful sentence !";
        let number_of_sentences = CHUNK_SIZE * 3 / beautiful_sentence.len();
        for _ in 0..number_of_sentences {
            fake_read += beautiful_sentence;
        }

        let mut stream = ElasticBufferStreamer::unlimited(fake_read.as_bytes());

        let first_sentence_of_next_chunk_dist =
            CHUNK_SIZE + beautiful_sentence.len() - CHUNK_SIZE % beautiful_sentence.len();
        for _ in 0..first_sentence_of_next_chunk_dist {
            assert!(stream.next().is_ok());
        }

        assert_eq!(stream.next(), Ok('T'));
        assert_eq!(stream.next(), Ok('h'));
        assert_eq!(stream.next(), Ok('i'));
        assert_eq!(stream.next(), Ok('s'));
        assert_eq!(stream.next(), Ok(' '));

        let cp = stream.checkpoint();

        assert_eq!(stream.next(), Ok('i'));
        assert_eq!(stream.next(), Ok('s'));
        assert_eq!(stream.next(), Ok(' '));

        stream.reset(cp);
        let cp = stream.checkpoint();

        assert_eq!(stream.next(), Ok('i'));
        assert_eq!(stream.next(), Ok('s'));
        assert_eq!(stream.next(), Ok(' '));

        for _ in 0..first_sentence_of_next_chunk_dist {
            assert!(stream.next().is_ok());
        }

        assert_eq!(stream.next(), Ok('a'));

        stream.reset(cp);

        assert_eq!(stream.next(), Ok('i'));
        assert_eq!(stream.next(), Ok('s'));
        assert_eq!(stream.next(), Ok(' '));
    }

    #[test]
    fn it_frees_useless_memory_when_reading_new_chunk() {
        let mut fake_read = String::with_capacity(CHUNK_SIZE * 3);

        let beautiful_sentence = "This is a sentence, what a beautiful sentence !";
        let number_of_sentences = CHUNK_SIZE * 3 / beautiful_sentence.len();
        for _ in 0..number_of_sentences {
            fake_read += beautiful_sentence;
        }

        let mut stream = ElasticBufferStreamer::unlimited(fake_read.as_bytes());

        let cp = stream.checkpoint();
        assert_eq!(stream.buffer_len(), 0);
        assert_eq!(stream.next(), Ok('T'));
        assert_eq!(stream.buffer_len(), CHUNK_SIZE);

        for _ in 0..CHUNK_SIZE {
            assert!(stream.next().is_ok());
        }

        assert_eq!(stream.buffer_len(), 2 * CHUNK_SIZE);

        stream.reset(cp);

        assert_eq!(stream.next(), Ok('T'));

        for _ in 0..2 * CHUNK_SIZE {
            assert!(stream.next().is_ok());
        }

        assert!(stream.buffer_len() < 2 * CHUNK_SIZE);
    }

    #[test]
    fn it_gets_ranges() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);

        let cp1 = stream.checkpoint();

        for _ in 0..3 {
            stream.next().unwrap();
        }

        let cp2 = stream.checkpoint();

        for _ in 0..2 {
            stream.next().unwrap();
        }

        let rg1 = stream.range_from_checkpoint(cp1).to_ref(&mut stream);

        assert_eq!(rg1, &(b"This ")[..]);

        let rg2 = stream.range_from_checkpoint(cp2).to_ref(&mut stream);

        assert_eq!(rg2, &(b"s ")[..]);
    }

    #[test]
    fn it_increases_capacity_when_needed() {
        let final_capacity = CHUNK_SIZE * (INITIAL_CAPACITY + 2);
        let mut fake_read = String::with_capacity(final_capacity);

        let beautiful_sentence = "This is a sentence, what a beautiful sentence !";
        let number_of_sentences = final_capacity / beautiful_sentence.len();
        for _ in 0..number_of_sentences {
            fake_read += beautiful_sentence;
        }

        let mut stream = ElasticBufferStreamer::unlimited(fake_read.as_bytes());

        let cp = stream.checkpoint();

        for _ in 0..(number_of_sentences * beautiful_sentence.len()) {
            stream.next().unwrap();
        }

        let rg = stream.range_from_checkpoint(cp).to_ref(&mut stream);

        assert_eq!(rg.len(), number_of_sentences * beautiful_sentence.len());
    }
}
