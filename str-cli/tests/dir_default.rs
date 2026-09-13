//! CLI 行为测试：规范 §9 `[dir]` 缺省规则（v1.10.0）。
//!
//! 与 `commands.rs` 互补：那条链在 `cmd::` 层显式传路径；这里验证 **clap 层** ——
//! 全部 bundle 命令的 `<dir>` 位置参数可省略，缺省目标为当前工作目录；
//! `init` 缺省以当前路径为基准目标（未以 `.str` 结尾时自动追加）。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 运行一条 `str` 子命令（`cwd` 即省略 `<dir>` 时的工作目录），返回 `(退出码, 合并输出)`。
fn run(cwd: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_str"))
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("strdir-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    p
}

#[test]
fn dir_argument_defaults_to_current_directory() {
    let base = tmp("dirdefault");
    let cwd = base.join("proj");
    std::fs::create_dir_all(&cwd).unwrap();

    // `init` 缺省目标 = 当前路径 → 在 `proj/` 里执行创建的是兄弟目录 `proj.str`（而非 `..str`）
    let (code, out) = run(&cwd, &["init"]);
    assert_eq!(code, 0, "{out}");
    let bundle = base.join("proj.str");
    assert!(bundle.join("._meta").exists(), "{out}");
    assert!(bundle.join("._schema").exists(), "{out}");

    // 在 bundle 内省略 `<dir>`：目标即当前目录（读类 + 写类 + 门禁各验一例）
    let (code, out) = run(
        &bundle,
        &["node", "add", "--type", "a.b", "--title", "T", "--summary", "s"],
    );
    assert_eq!(code, 0, "{out}");
    for args in [
        vec!["tree"],
        vec!["show"],
        vec!["ls"],
        vec!["context"],
        vec!["norm"],
        vec!["validate", "--strict"],
        vec!["fmt", "--check"],
        vec!["sync"],
    ] {
        let (code, out) = run(&bundle, &args);
        assert_eq!(code, 0, "{args:?}: {out}");
    }

    // `spec set` 同样省略 `<dir>`；写完 `validate --strict` 仍须干净
    let (code, out) = run(&bundle, &["spec", "set", "1.7.0"]);
    assert_eq!(code, 0, "{out}");
    let (code, out) = run(&bundle, &["validate", "--strict"]);
    assert_eq!(code, 0, "{out}");

    // 显式路径照常工作（缺省值不改变原有语义）
    let (code, out) = run(&cwd, &["tree", bundle.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn init_without_dir_in_a_str_named_directory_is_rejected() {
    let base = tmp("initstr");
    let cwd = base.join("already.str");
    std::fs::create_dir_all(&cwd).unwrap();

    // 当前目录已以 `.str` 结尾 → 目标即自身，已存在则报错（该目录通常已是 bundle）
    let (code, out) = run(&cwd, &["init"]);
    assert_ne!(code, 0, "{out}");
    assert!(out.contains("已存在"), "{out}");

    let _ = std::fs::remove_dir_all(&base);
}
