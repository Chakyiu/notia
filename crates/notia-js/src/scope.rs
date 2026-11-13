//! Lends the `App` to host functions while a plugin's JavaScript runs.
//!
//! QuickJS calls host functions from inside `Context::with`, with no way to
//! hand them `&mut App`. Whoever starts running JavaScript already holds
//! `&mut App`: [`lend`] makes it available to [`with_app`] for exactly the
//! duration of that call.

use std::cell::{Cell, RefCell};

use gpui_kit::App;

thread_local! {
    static LENT: RefCell<Vec<*mut App>> = const { RefCell::new(Vec::new()) };
    static BORROWED: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f`, during which [`with_app`] can borrow `cx`.
pub(crate) fn lend<R>(cx: &mut App, f: impl FnOnce() -> R) -> R {
    struct Return {
        was_borrowed: bool,
    }
    impl Drop for Return {
        fn drop(&mut self) {
            LENT.with_borrow_mut(|lent| lent.pop());
            BORROWED.set(self.was_borrowed);
        }
    }

    LENT.with_borrow_mut(|lent| lent.push(std::ptr::from_mut(cx)));
    // A host function holding the outer `App` can lend it again, for
    // example to start another plugin; the new loan is the only one in use
    // until it returns.
    let _return = Return {
        was_borrowed: BORROWED.replace(false),
    };
    f()
}

/// Borrows the `App` of the innermost [`lend`]. Returns `None` outside of
/// `lend`, and while the `App` is already borrowed.
pub(crate) fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    let app = LENT.with_borrow(|lent| lent.last().copied())?;
    if BORROWED.replace(true) {
        return None;
    }
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            BORROWED.set(false);
        }
    }
    let _release = Release;
    // SAFETY: `app` comes from the `&mut App` that `lend` holds, and does not
    // use, until `f` returns, and `BORROWED` keeps this the only reference
    // made from it.
    Some(f(unsafe { &mut *app }))
}
