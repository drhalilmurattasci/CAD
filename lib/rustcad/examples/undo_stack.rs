//! Walk through apply / undo / redo on a generic `CommandStack`.
//!
//! The stack is parameterized over both the target document and the
//! error family, so this example spins up a toy `TextDoc` with a
//! `TextError` enum — nothing about `rustcad::undo` knows about
//! scenes, entities, or any other engine-specific concept.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example undo_stack
//! ```

use rustcad::undo::{Command, CommandStack};

#[derive(Default, Debug)]
struct TextDoc {
    buffer: String,
}

#[derive(Debug)]
enum TextError {
    #[allow(dead_code)]
    Empty,
}

struct Append {
    text:    String,
    applied: usize,
}

impl Append {
    fn new(text: impl Into<String>) -> Self {
        Self {
            text:    text.into(),
            applied: 0,
        }
    }
}

impl Command<TextDoc, TextError> for Append {
    fn label(&self) -> &'static str {
        "append"
    }

    fn apply(&mut self, doc: &mut TextDoc) -> Result<(), TextError> {
        doc.buffer.push_str(&self.text);
        self.applied = self.text.len();
        Ok(())
    }

    fn undo(&mut self, doc: &mut TextDoc) -> Result<(), TextError> {
        let new_len = doc.buffer.len().saturating_sub(self.applied);
        doc.buffer.truncate(new_len);
        Ok(())
    }
}

fn main() {
    let mut doc = TextDoc::default();
    let mut stack: CommandStack<TextDoc, TextError> = CommandStack::default();

    stack.execute(&mut doc, Box::new(Append::new("Hello"))).unwrap();
    stack.execute(&mut doc, Box::new(Append::new(", world!"))).unwrap();
    println!("after 2 edits: {:?}", doc.buffer);

    stack.undo(&mut doc).unwrap();
    println!("after 1 undo:  {:?}", doc.buffer);

    stack.redo(&mut doc).unwrap();
    println!("after redo:    {:?}", doc.buffer);

    println!(
        "stack depth: undo={}, redo={}",
        stack.undo_len(),
        stack.redo_len()
    );
}
