#[path = "stdio/actions.rs"]
#[cfg(any())]
mod actions;
#[path = "stdio/completion.rs"]
#[cfg(any())]
mod completion;
#[path = "stdio/current_protocol.rs"]
mod current_protocol;
#[path = "stdio/diagnostics.rs"]
#[cfg(any())]
mod diagnostics;
#[path = "stdio/folding.rs"]
#[cfg(any())]
mod folding;
#[path = "stdio/formatting.rs"]
#[cfg(any())]
mod formatting;
#[path = "stdio/lifecycle.rs"]
mod lifecycle;
#[path = "stdio/navigation.rs"]
mod navigation;
#[path = "stdio/rename.rs"]
mod rename;
#[path = "stdio/search.rs"]
mod search;
mod support;
