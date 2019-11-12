use core::num::NonZeroUsize;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io::Read;
use std::rc::{Rc, Weak};


const ITEM_INDEX_SIZE: usize = 13;
const ITEM_INDEX_MASK: usize = (1 << ITEM_INDEX_SIZE) - 1;
pub const CHUNK_SIZE: usize = 1 << ITEM_INDEX_SIZE;

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
            let handled = cp.0.upgrade().unwrap(); // sub_offset has to be called right after min, so there is no unhandled value
            handled.set(handled.get() - value);
        }
    }
}


fn read_exact_or_eof<R: Read>(
    reader: &mut R,
    mut chunk: &mut [u8],
) -> std::io::Result<Option<NonZeroUsize>> {
    while !chunk.is_empty() {
        match reader.read(chunk) {
            Ok(0) => break,
            Ok(n) => {
                let tmp = chunk;
                chunk = &mut tmp[n..];
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }

    Ok(NonZeroUsize::new(chunk.len()))
}


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


pub struct Streamer<R: Read> {
    raw_read: R,
    buffer: VecDeque<[u8; CHUNK_SIZE]>,
    eof: Option<NonZeroUsize>,
    checkpoints: CheckPointSet,
    cursor_pos: usize,
    offset: u64, // The capacity of this parameter limits the size of the stream
}

impl<R: Read> Streamer<R> {
    pub fn new(read: R) -> Self {
        Self {
            raw_read: read,
            buffer: VecDeque::new(),
            eof: None,
            checkpoints: CheckPointSet::new(),
            cursor_pos: 0,
            offset: 0,
        }
    }

    fn chunk_index(&self) -> usize {
        self.cursor_pos >> ITEM_INDEX_SIZE
    }

    fn item_index(&self) -> usize {
        self.cursor_pos & ITEM_INDEX_MASK
    }

    fn free_useless_chunks(&mut self) {
        let checkpoint_pos_min = self.checkpoints.min_checkpoint_position();
        let global_pos_min =
            checkpoint_pos_min.map_or(self.cursor_pos, |cp_min| cp_min.min(self.cursor_pos));
        let drain_quantity = global_pos_min / CHUNK_SIZE;

        self.buffer.drain(..drain_quantity);

        let offset_delta = drain_quantity * CHUNK_SIZE;
        self.cursor_pos -= offset_delta;
        self.offset += offset_delta as u64;
        self.checkpoints.sub_offset(offset_delta);
    }

    pub fn buffer_len(&self) -> usize {
        self.buffer.len()
    }

    pub fn next(&mut self) -> Result<u8, StreamError> {
        assert!(self.chunk_index() <= self.buffer.len());

        if self.chunk_index() == self.buffer.len() {
            assert!(self.eof.is_none());
            self.free_useless_chunks();
            self.buffer.push_back([0; CHUNK_SIZE]);
            self.eof = read_exact_or_eof(&mut self.raw_read, self.buffer.back_mut().unwrap())?;
        }

        if self.chunk_index() == self.buffer.len() - 1 {
            if let Some(eof_pos_from_right) = self.eof {
                if self.item_index() >= CHUNK_SIZE - eof_pos_from_right.get() {
                    return Err(StreamError::EndOfInput);
                }
            }
        }

        let chunk = self.buffer.get(self.chunk_index()).unwrap(); // We can unwrap because self.chunk_index() < self.buffer.len()
        let item = chunk[self.item_index()]; //  item_index < CHUNK_SIZE
        self.cursor_pos += 1;

        Ok(item)
    }

    pub fn position(&self) -> u64 {
        self.offset + self.cursor_pos as u64
            
    }

    pub fn checkpoint(&self) -> CheckPoint {
        self.checkpoints.insert(self.cursor_pos)
    }

    pub fn reset(&mut self, checkpoint: CheckPoint) {
        self.cursor_pos = checkpoint.inner();
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_next_on_one_chunk() {
        let fake_read = &b"This is the text !"[..];
        let mut stream = Streamer::new(fake_read);
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

        let mut stream = Streamer::new(fake_read.as_bytes());

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

        let mut stream = Streamer::new(fake_read.as_bytes());

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

        let mut stream = Streamer::new(fake_read.as_bytes());

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
