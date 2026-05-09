# Colab (Code Lab)

A powerful CLI tool for automated code refactoring and updates at scale.

## Overview

Colab (Code Lab) is a command-line tool written in Rust that enables scripted code refactoring at scale. It uses tree-sitter for precise language parsing and provides a domain-specific language for defining refactoring operations.

Currently, Colab specializes in Go module dependency updates, allowing you to replace import statements across your entire codebase with a single command.

## Features

- **Go Import Refactoring**: Replace Go import statements across your codebase
- **Scripted Refactoring**: Define refactoring operations using a simple domain-specific language
- **Language Server**: Includes a language server for integration with IDEs
- **Tree-sitter Integration**: Uses tree-sitter for accurate code parsing

## Installation

### Prerequisites

- Rust 1.70 or later (check with `rustc --version`)
- Cargo (comes with Rust)

### Building from Source

1. Clone the repository:

   ```sh
   git clone https://github.com/grahambrooks/colab.git
   cd colab
   ```

2. Build the project:

   ```sh
   cargo build --release
   ```

3. The executable will be available at `target/release/colab`

## Usage

### Refactoring Go Imports

To replace Go import statements:

```sh
colab refactor --script path/to/script.codemod [directories or files]
```

Example:

```sh
colab refactor --script examples/go/imports/simple.codemod .
```

You can also change the working directory before processing:

```sh
colab refactor -C /path/to/working/directory --script path/to/script.codemod .
```

### Starting the Language Server

To start the language server:

```sh
colab server [--port PORT]
```

The default port is 8080.

## Codemod Script Syntax

Codemod scripts use a simple domain-specific language to define refactoring operations. Here's an example:

```
refactor "name" {
    match go::import "old.module.path" {
        replace "new.module.path"
    }
}
```

This script will replace all imports of "old.module.path" with "new.module.path" in Go files.

Targeting a namespace that colab doesn't implement (e.g. `rust::module`) produces a clear `unsupported operation` error instead of silently doing nothing.

## Project Layout

A high-level walk through the source tree, the runtime data flow, and how to add a new transformation lives in [ARCHITECTURE.md](ARCHITECTURE.md).

## Examples

The repository includes examples in the `examples` directory:

- `examples/go/imports/main.go`: A simple Go file with imports
- `examples/go/imports/simple.codemod`: A codemod script to replace imports

To run the example:

```sh
colab refactor -C examples/go/imports --script simple.codemod .
```

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

This project is open source and available under the [MIT License](LICENSE).
