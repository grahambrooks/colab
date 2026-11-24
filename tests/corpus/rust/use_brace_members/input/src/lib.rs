use serde::{Deserialize, Serialize};
use std::fmt::{self, Debug};
use std::io::{
    Read,
    Write,
};
use tree_sitter::{Parser, Tree};

pub fn parse(tree: Tree) -> Tree {
    tree
}
