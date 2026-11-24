//! `--stdin`: one source on standard input, the result on standard output.

use super::*;

/// Resolve the effective exec mode given explicit flags and the
/// format-specific defaults.
pub(super) fn resolve_exec_mode(
    write: bool,
    dry_run: bool,
    check: bool,
    format: Format,
    stdout_is_tty: bool,
) -> ExecMode {
    if check {
        ExecMode::Check
    } else if write {
        ExecMode::Write
    } else if dry_run {
        ExecMode::DryRun
    } else {
        format.default_exec_mode(stdout_is_tty)
    }
}

pub(super) fn run_stdin(args: RefactorArgs) -> Result<u8> {
    let path = args
        .stdin_path
        .as_ref()
        .expect("clap requires --path with --stdin")
        .clone();

    if args.script.reads_stdin() {
        return Err(Error::Config(
            "`--script -` and `--stdin` both read stdin; pass the script as a file or with `-e`"
                .into(),
        ));
    }
    let backends = default_backends();
    let mut refactoring = args.script.compile(&backends)?;
    args.select.apply(&mut refactoring)?;
    let _oracle = OracleGuard::install(&refactoring, args.lsp.as_deref(), args.lsp_timeout)?;
    refuse_phased(&refactoring, &args)?;
    // The rules are in every report; the log names the script, so an
    // agent does not read each rule twice.
    info!(
        "Running `{}` ({} rule(s))",
        refactoring.name,
        refactoring.rules.len()
    );
    log::debug!("Rules: {refactoring}");

    let mut source = String::new();
    io::stdin()
        .read_to_string(&mut source)
        .map_err(io_to_error)?;

    let (change, mut report) = codemod::pipeline::run_text(&refactoring, path, source)?;

    match args.format {
        Format::Human => {
            // The transformed source *is* the output for `--stdin`; the
            // report would corrupt it, so it is deliberately omitted.
            let mut out = io::stdout().lock();
            out.write_all(change.after.as_bytes())
                .map_err(io_to_error)?;
        }
        Format::Json | Format::Ndjson | Format::Diff | Format::Sarif | Format::Github => {
            let mut reporter =
                format::make_reporter(args.format, ExecMode::DryRun, args.render_options());
            attribute_lines(
                &args,
                &refactoring,
                std::slice::from_ref(&change),
                &mut report,
                reporter.as_mut(),
            );
            reporter
                .report(&change)
                .map_err(|e| Error::io_at(&change.path, e))?;
            reporter.finish(&report).map_err(io_to_error)?;
        }
    }

    Ok(if args.check && change.changed() {
        10
    } else {
        0
    })
}
