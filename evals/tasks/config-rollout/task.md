# Roll out a minimum toolchain

Every crate declares `rust-version = "1.85"` in its `[package]` table,
and every web package requires Node 20 or later (`engines.node`).
Leave everything else, including formatting and comments, as it is.
