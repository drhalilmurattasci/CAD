//! Generic do / undo / redo stack — the machinery, not the commands.
//!
//! Lifted out of `engine::commands` so any tool that needs linear
//! undo over *some* document type can drop it in without dragging the
//! engine's scene / ECS in along with it. Concrete commands
//! (`RenameEntityCommand`, `NudgeTransformCommand`, …) live wherever
//! the *target* type they mutate lives — the stack itself doesn't
//! care.
//!
//! # Design
//!
//! One trait, one struct:
//!
//! - [`Command<T, E>`] — "apply this change to `T`, and remember how to
//!   roll it back." Implementors store whatever state they need to
//!   undo (usually a snapshot of the field they're about to change).
//! - [`CommandStack<T, E>`] — linear undo/redo history. Holds
//!   `Box<dyn Command<T, E>>` entries so the trait stays object-safe;
//!   `execute` runs and records, `undo` / `redo` walk the history.
//!
//! Both generics are load-bearing:
//!
//! - `T` is the target document (`SceneDocument`, `TextBuffer`,
//!   `ProjectTree`, …).
//! - `E` is the error family. No default — consumers pick their own,
//!   typically a crate-local `CommandError` enum, so the stack
//!   surfaces domain-specific failure modes instead of a
//!   `Box<dyn Error>` soup.
//!
//! # Example
//!
//! ```
//! use rustcad::undo::{Command, CommandStack};
//!
//! #[derive(Default)]
//! struct Counter(i32);
//!
//! struct Increment {
//!     by: i32,
//!     applied: bool,
//! }
//!
//! impl Command<Counter, &'static str> for Increment {
//!     fn label(&self) -> &'static str { "increment" }
//!     fn apply(&mut self, c: &mut Counter) -> Result<(), &'static str> {
//!         c.0 += self.by;
//!         self.applied = true;
//!         Ok(())
//!     }
//!     fn undo(&mut self, c: &mut Counter) -> Result<(), &'static str> {
//!         c.0 -= self.by;
//!         self.applied = false;
//!         Ok(())
//!     }
//! }
//!
//! let mut counter = Counter::default();
//! let mut stack: CommandStack<Counter, &'static str> = CommandStack::default();
//!
//! stack.execute(&mut counter, Box::new(Increment { by: 5, applied: false })).unwrap();
//! assert_eq!(counter.0, 5);
//!
//! stack.undo(&mut counter).unwrap();
//! assert_eq!(counter.0, 0);
//!
//! stack.redo(&mut counter).unwrap();
//! assert_eq!(counter.0, 5);
//! ```

/// A reversible operation on a `T`.
///
/// Implementors are responsible for remembering enough state during
/// [`apply`](Command::apply) that the subsequent
/// [`undo`](Command::undo) restores the target exactly — no FP drift,
/// no lost side-information. The usual shape: snapshot the
/// affected field(s) on first apply, write them back on undo.
///
/// [`label`](Command::label) is used by UI layers (undo-history
/// panels, debug logs) and should describe the *user-visible*
/// operation (`"rename entity"`, `"move transform"`), not the
/// implementation.
pub trait Command<T, E> {
    /// Human-readable operation name. Shown in undo-history UIs and
    /// logs. Prefer short, lowercase, imperative phrases.
    fn label(&self) -> &'static str;

    /// Apply this command to `target`. Called once by
    /// [`CommandStack::execute`] and again on every subsequent
    /// [`CommandStack::redo`] — implementors must handle both paths.
    fn apply(&mut self, target: &mut T) -> Result<(), E>;

    /// Reverse a prior [`apply`](Command::apply). Called by
    /// [`CommandStack::undo`]. Implementors should assume the most
    /// recent `apply` succeeded; pairing an `undo` with state that
    /// never went through `apply` is a caller bug.
    fn undo(&mut self, target: &mut T) -> Result<(), E>;
}

/// Linear undo / redo history over a `T`, failing with `E`.
///
/// `execute` pushes a new command onto the undo stack and clears the
/// redo stack (classic "editing invalidates redo" semantics). `undo`
/// pops the most recent command and moves it to the redo stack;
/// `redo` reverses that move.
pub struct CommandStack<T, E> {
    undo_stack: Vec<Box<dyn Command<T, E>>>,
    redo_stack: Vec<Box<dyn Command<T, E>>>,
}

impl<T, E> Default for CommandStack<T, E> {
    fn default() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }
}

impl<T, E> CommandStack<T, E> {
    /// Run `command` against `target` and, on success, record it so it
    /// can later be undone. Clears the redo stack — re-doing a
    /// command after new edits would produce inconsistent history.
    ///
    /// On failure (the command's `apply` returned `Err`), nothing is
    /// recorded and the stack is unchanged.
    pub fn execute(
        &mut self,
        target: &mut T,
        mut command: Box<dyn Command<T, E>>,
    ) -> Result<(), E> {
        command.apply(target)?;
        self.undo_stack.push(command);
        self.redo_stack.clear();
        Ok(())
    }

    /// Roll back the most recent command. Returns `Ok(false)` when the
    /// stack is empty (there was nothing to undo) — that's a normal
    /// UI state, not an error. `Ok(true)` on a successful undo;
    /// `Err(E)` if the command's `undo` surfaced an error.
    pub fn undo(&mut self, target: &mut T) -> Result<bool, E> {
        let Some(mut command) = self.undo_stack.pop() else {
            return Ok(false);
        };

        command.undo(target)?;
        self.redo_stack.push(command);
        Ok(true)
    }

    /// Re-apply the most recently undone command. Returns `Ok(false)`
    /// when the redo stack is empty.
    pub fn redo(&mut self, target: &mut T) -> Result<bool, E> {
        let Some(mut command) = self.redo_stack.pop() else {
            return Ok(false);
        };

        command.apply(target)?;
        self.undo_stack.push(command);
        Ok(true)
    }

    /// How many commands are currently on the undo stack. Useful for
    /// status-bar displays (`5 / 3` history indicators) and tests.
    pub fn undo_len(&self) -> usize {
        self.undo_stack.len()
    }

    /// How many commands are currently on the redo stack.
    pub fn redo_len(&self) -> usize {
        self.redo_stack.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default, Debug, PartialEq, Eq)]
    struct Counter(i32);

    struct Add(i32);

    impl Command<Counter, ()> for Add {
        fn label(&self) -> &'static str { "add" }
        fn apply(&mut self, c: &mut Counter) -> Result<(), ()> {
            c.0 += self.0;
            Ok(())
        }
        fn undo(&mut self, c: &mut Counter) -> Result<(), ()> {
            c.0 -= self.0;
            Ok(())
        }
    }

    #[test]
    fn execute_applies_and_records() {
        let mut c = Counter::default();
        let mut s: CommandStack<Counter, ()> = CommandStack::default();
        s.execute(&mut c, Box::new(Add(3))).unwrap();
        assert_eq!(c, Counter(3));
        assert_eq!(s.undo_len(), 1);
        assert_eq!(s.redo_len(), 0);
    }

    #[test]
    fn undo_redo_roundtrip() {
        let mut c = Counter::default();
        let mut s: CommandStack<Counter, ()> = CommandStack::default();
        s.execute(&mut c, Box::new(Add(3))).unwrap();
        s.execute(&mut c, Box::new(Add(2))).unwrap();
        assert_eq!(c, Counter(5));

        assert!(s.undo(&mut c).unwrap());
        assert_eq!(c, Counter(3));
        assert!(s.undo(&mut c).unwrap());
        assert_eq!(c, Counter(0));
        // Nothing left to undo.
        assert!(!s.undo(&mut c).unwrap());

        assert!(s.redo(&mut c).unwrap());
        assert_eq!(c, Counter(3));
        assert!(s.redo(&mut c).unwrap());
        assert_eq!(c, Counter(5));
        assert!(!s.redo(&mut c).unwrap());
    }

    #[test]
    fn new_execute_clears_redo() {
        let mut c = Counter::default();
        let mut s: CommandStack<Counter, ()> = CommandStack::default();
        s.execute(&mut c, Box::new(Add(3))).unwrap();
        s.undo(&mut c).unwrap();
        assert_eq!(s.redo_len(), 1);

        s.execute(&mut c, Box::new(Add(7))).unwrap();
        assert_eq!(s.redo_len(), 0);
        assert_eq!(c, Counter(7));
    }
}
