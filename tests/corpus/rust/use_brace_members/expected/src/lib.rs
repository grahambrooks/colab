use serde::Deserialize;
use serde_json::Value;
use std::fmt::{self, Display};
use std::io::{
    Read,
    Write,
};
use tree_sitter::Tree;

pub fn parse(tree: Tree) -> Tree {
    tree
}
