use owning_ref::OwningHandle;
use slice_deque::SliceDeque;
use std::cell::{Cell, RefCell};
use std::io::Read;
use std::marker::PhantomData;
use std::rc::{Rc, Weak};
use std::str::CharIndices;

use super::Streamer;
use super::StreamerError;
use super::StreamerRange;

const ITEM_INDEX_SIZE: usize = 13;
const ITEM_INDEX_MASK: usize = (1 << ITEM_INDEX_SIZE) - 1;
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

pub struct ElasticBufferStreamer<R: Read> {
    raw_read: R,
    buffer: OwningHandle<Box<SliceDeque<u8>>, CharIndices<'static>>,
    max_size: Option<usize>,
    eof: bool,
    non_complete_char_len: u8,
    cursor_pos: usize,
    before_pos: Option<usize>,
    checkpoints: CheckPointSet,
    offset: u64, // The capacity of this parameter limits the length of the stream
}

impl<R: Read> ElasticBufferStreamer<R> {
    fn with_maybe_max_size(read: R, max_size: Option<usize>) -> Self {
        Self {
            raw_read: read,
            buffer: OwningHandle::new_with_fn(Box::new(SliceDeque::with_capacity(INITIAL_CAPACITY)), |_buffer| &"".char_indices()),
            max_size,
            eof: false,
            non_complete_char_len: 0,
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

    fn raw_buffered_len(&self) -> usize {
        self.buffer.as_owner().len() - CHUNK_SIZE - self.non_complete_char_len as usize
    }

    /// UNSOUND : `new_cursor_pos` has to be the begin of a string.
    fn set_cursor_pos(&mut self, new_cursor_pos: usize) {
        self.buffer = OwningHandle::new_with_fn(self.buffer.to_owner(), |buffer| unsafe { std::str::from_utf8_unchecked(buffer.as_slice())[new_cursor_pos..self.raw_buffered_len()].char_indices() });
        self.cursor_pos = new_cursor_pos;
    }

    fn read_new_chunk_from_raw(&mut self) -> std::io::Result<()>{
        let buffer = self.buffer.to_owner();
        let io_result = match self.raw_read.read(buffer[self.raw_buffered_len()..]) {
            Ok(0) => {
                self.eof = true;
                Ok()
            },
            Ok(n) => {
                buffer.extend(std::iter::repeat(0).take(CHUNK_SIZE - n));
                Ok()
            },
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => self.read_new_chunk_from_raw(),
            Err(e) => return Err(e),
        };
    }

    fn free_useless_data(&mut self) {
        let checkpoint_pos_min = self.checkpoints.min_checkpoint_position();
        let global_pos_min =
            checkpoint_pos_min.map_or(self.cursor_pos, |cp_min| cp_min.min(self.cursor_pos));

        self.buffer.drain(..global_pos_min);

        self.cursor_pos -= global_pos_min;
        self.before_pos.map(|before_pos_val| before_pos_val - global_pos_min);
        self.offset += global_pos_min as u64;
        self.checkpoints.sub_offset(global_pos_min);
    }

    pub fn buffer_len(&self) -> usize {
        self.buffer.len()
    }

    fn range_ref_from_to_checkpoint(&mut self, from_cp: CheckPoint, to_cp: CheckPoint) -> &[u8] {
        let range_begin = from_cp.inner();
        let range_end = to_cp.inner();

        let slice = self.buffer.as_slice();

        slice[range_begin..range_end]
    }
}

impl<R: Read> Streamer for ElasticBufferStreamer<R> {
    type CheckPoint = CheckPoint;
    type Range = Range<R>;

    fn next(&mut self) -> Result<u8, StreamerError> {
        if let (cursor_pos, Some(c)) = (*self.buffer).next() {
            self.before_pos = Some(self.cursor_pos);
            self.cursor_pos = cursor_pos;
            c
        } else {
            if !self.eof {
                self.free_useless_data();
                self.read_new_chunk_from_raw()?;
                self.next()
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

    #[test]
    fn it_goes_next_on_one_chunk() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = ElasticBufferStreamer::unlimited(fake_read);
        assert_eq!(stream.next(), Ok(b'T'));
        assert_eq!(stream.next(), Ok(b'h'));
        assert_eq!(stream.next(), Ok(b'i'));
        assert_eq!(stream.next(), Ok(b's'));
        assert_eq!(stream.next(), Ok(b' '));

        for _ in 0..12 {
            assert!(stream.next().is_ok());
        }

        assert_eq!(stream.next(), Ok(b'!'));
        assert_eq!(stream.next(), Err(StreamerError::EndOfInput));
    }

    #[test]
    fn it_goes_next_on_multiple_chunks() {
        let mut fake_read = String::with_capacity(CHUNK_SIZE * 3);

        let beautiful_sentence = "This is a sentence, what a beautiful sentence !";
        let number_of_sentences = CHUNK_SIZE * 3 / beautiful_sentence.len();
        for _ in 0..number_of_sentences {
            fake_read += beautiful_sentence;
        }

        let mut stream = ElasticBufferStreamer::unlimited(fake_read.as_bytes());

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
