//! Reading a tool call's arguments. Each reader names the argument and
//! what it must be, as a protocol error, so a caller can fix the call.

use super::*;

pub(crate) fn arg_str(args: &Value, key: &str) -> Result<String, ToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| ToolError::protocol(format!("missing string argument `{key}`")))
}

pub(crate) fn arg_bool(args: &Value, key: &str) -> Result<bool, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(ToolError::protocol(format!(
            "argument `{key}` must be a boolean"
        ))),
    }
}

pub(crate) fn arg_opt_path(args: &Value, key: &str) -> Result<Option<PathBuf>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(PathBuf::from(s))),
        Some(_) => Err(ToolError::protocol(format!(
            "argument `{key}` must be a string"
        ))),
    }
}

pub(crate) fn arg_usize(args: &Value, key: &str, default: usize) -> Result<usize, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .map(|n| usize::try_from(n).unwrap_or(usize::MAX))
            .ok_or_else(|| ToolError::protocol(format!("argument `{key}` must be a number"))),
    }
}

pub(crate) fn arg_render_options(args: &Value) -> Result<RenderOptions, ToolError> {
    let detail = match args.get("detail") {
        None | Some(Value::Null) => Detail::default(),
        Some(Value::String(s)) => Detail::parse(s).ok_or_else(|| {
            ToolError::protocol(format!(
                "unknown detail level `{}`; expected one of: {}",
                s,
                Detail::NAMES.join(", ")
            ))
        })?,
        Some(_) => return Err(ToolError::protocol("argument `detail` must be a string")),
    };
    Ok(RenderOptions {
        detail,
        max_files: arg_usize(args, "max_files", render::DEFAULT_MAX_FILES)?,
        max_diff_bytes: arg_usize(args, "max_diff_bytes", render::DEFAULT_MAX_DIFF_BYTES)?,
    })
}

/// Resolve the `paths` argument, rooting relative entries at `cwd`.
pub(crate) fn arg_paths(
    args: &Value,
    key: &str,
    cwd: Option<&Path>,
) -> Result<Vec<PathBuf>, ToolError> {
    let value = args
        .get(key)
        .ok_or_else(|| ToolError::protocol(format!("missing argument `{key}`")))?;
    let array = value.as_array().ok_or_else(|| {
        ToolError::protocol(format!("argument `{key}` must be an array of strings"))
    })?;
    let mut out = Vec::with_capacity(array.len());
    for item in array {
        let s = item
            .as_str()
            .ok_or_else(|| ToolError::protocol(format!("argument `{key}` must contain strings")))?;
        let path = PathBuf::from(s);
        out.push(match cwd {
            Some(dir) if path.is_relative() => dir.join(path),
            _ => path,
        });
    }
    if out.is_empty() {
        return Err(ToolError::protocol(format!(
            "argument `{key}` cannot be empty"
        )));
    }
    Ok(out)
}
