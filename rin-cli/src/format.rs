use crate::{Discovery, arguments::FmtOptions, discover};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read, Write},
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

pub fn run(options: &FmtOptions) -> Result<bool, String> {
    if options.stdin {
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .map_err(|e| format!("stdin: {e}"))?;
        let output =
            rin::fmt::format(&source).map_err(|e| format!("stdin: {}", e.render(&source)))?;
        if options.check {
            if source != output {
                println!("-");
                return Ok(false);
            }
        } else {
            io::stdout()
                .lock()
                .write_all(output.as_bytes())
                .map_err(|e| format!("stdout: {e}"))?;
        }
        return Ok(true);
    }
    let mut files = BTreeSet::new();
    for path in &options.paths {
        discover(path, Discovery::All, true, &mut files)?;
    }
    // すべての構文とAST同一性を検査してから、最初のファイルを書き換える。
    let mut changed = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let output = rin::fmt::format(&source)
            .map_err(|e| format!("{}: {}", path.display(), e.render(&source)))?;
        if source != output {
            changed.push((path, source, output));
        }
    }
    if options.check {
        for (path, _, _) in &changed {
            println!("{}", path.display());
        }
        return Ok(changed.is_empty());
    }
    for (path, source, output) in changed {
        replace(&path, &source, &output).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(true)
}

static NEXT: AtomicUsize = AtomicUsize::new(0);
fn replace(path: &Path, source: &str, output: &str) -> io::Result<()> {
    if fs::read_to_string(path)? != source {
        return Err(io::Error::other("source changed while formatting"));
    }
    let permissions = fs::metadata(path)?.permissions();
    if permissions.readonly() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "source is read-only",
        ));
    }
    let temporary = path.with_file_name(format!(
        ".rin-fmt-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(output.as_bytes())?;
        file.set_permissions(permissions)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
