# colab
Code Lab for automated refactoring and updates

## CLI Tool for Go Module Dependency Updates

This repository includes a CLI tool written in Rust that scans all Go code and replaces module dependency references as specified in a YAML file. The tool uses tree-sitter for language parsing.

### Usage

1. Ensure you have Rust installed on your system. If not, you can install it from [rust-lang.org](https://www.rust-lang.org/).

2. Clone this repository and navigate to the `cli` directory:

   ```sh
   git clone https://github.com/grahambrooks/colab.git
   cd colab/cli
   ```

3. Build the CLI tool:

   ```sh
   cargo build --release
   ```

4. Create a `config.yaml` file in the `cli` directory with the following structure:

   ```yaml
   replace:
       go-module:
           from: some.module
           to: another.module
   ```

5. Run the CLI tool:

   ```sh
   ./target/release/cli --config config.yaml --path /path/to/go/code
   ```

   Replace `/path/to/go/code` with the path to the directory containing your Go code.

The CLI tool will scan all Go files in the specified directory and replace module dependency references as specified in the `config.yaml` file.
