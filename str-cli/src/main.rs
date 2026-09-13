//! `str` CLI 入口。

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use str_format::cmd;
use str_format::error::{Error, Result};

#[derive(Parser)]
#[command(
    name = "str",
    version,
    about = "STR 结构化树资源格式（.str）工具链",
    long_about = "STR 是目录 bundle 式的结构化树资源格式：深度 1 为独立节点，深度 ≥2 为关联分支，\n任意层级均可承载任意文件；每个分支的 `._meta`（TOML）记录元信息与内容清单。"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 创建新的 .str bundle（生成 ROOT `._meta` 与 `._schema/`）
    Init {
        /// 目标目录（未以 .str 结尾时自动追加）
        dir: PathBuf,
        /// bundle 短名
        #[arg(long)]
        name: Option<String>,
        /// 标题
        #[arg(long)]
        title: Option<String>,
        /// 一句话摘要
        #[arg(long)]
        summary: Option<String>,
    },
    /// 全量校验（规范第 6 章全部错误码）
    Validate {
        /// bundle 目录
        dir: PathBuf,
        /// 把告警也视为失败（CI 用）
        #[arg(long)]
        strict: bool,
        /// 输出 JSON
        #[arg(long)]
        json: bool,
        /// 校验前先执行一次 sync 修正清单
        #[arg(long)]
        fix_manifest: bool,
    },
    /// 渲染分支树（含跨枝关联线）
    Tree {
        /// bundle 目录
        dir: PathBuf,
        /// 最大渲染深度
        #[arg(long)]
        depth: Option<usize>,
        /// 显示 `refs` 关联线
        #[arg(long)]
        show_refs: bool,
    },
    /// 列出某分支的内容清单
    Ls {
        /// bundle 目录
        dir: PathBuf,
        /// 指定分支 id（缺省为 ROOT）
        #[arg(long)]
        uuid: Option<String>,
        /// 直接读磁盘而非读清单
        #[arg(long)]
        raw: bool,
    },
    /// 打印某分支的 `._meta`（归一化 JSON）
    Show {
        /// bundle 目录
        dir: PathBuf,
        /// 分支 id
        uuid: String,
        /// 附带 payload 正文
        #[arg(long)]
        full: bool,
    },
    /// 独立节点操作（深度 1）
    Node {
        #[command(subcommand)]
        cmd: NodeCmd,
    },
    /// 关联分支操作（任意深度）
    Branch {
        #[command(subcommand)]
        cmd: BranchCmd,
    },
    /// 跨枝关联线操作
    Ref {
        #[command(subcommand)]
        cmd: RefCmd,
    },
    /// 用磁盘实际状态修正 `entries` 与指纹
    Sync {
        /// bundle 目录
        dir: PathBuf,
        /// 只显示将要发生的变更
        #[arg(long)]
        dry_run: bool,
    },
    /// 按规范键序 / 表序重写 `._meta`（保注释）
    Fmt {
        /// bundle 目录
        dir: PathBuf,
        /// 只检查是否需要规范化
        #[arg(long)]
        check: bool,
        /// 丢弃整行注释（默认保留）
        #[arg(long)]
        strip_comments: bool,
    },
    /// 输出归一化 JSON（供外部 Schema 工具 / AI 使用）
    Norm {
        /// bundle 目录
        dir: PathBuf,
        /// 分支 id（缺省为 ROOT）
        uuid: Option<String>,
    },
    /// 生成供 AI 使用的上下文片段
    Context {
        /// bundle 目录
        dir: PathBuf,
        /// 分支 id
        uuid: String,
        /// 下钻层数
        #[arg(long, default_value_t = 2)]
        depth: usize,
        /// 字符预算
        #[arg(long, default_value_t = 8000)]
        budget: usize,
    },
    /// 导出为单一文件（只读、派生）
    Export {
        /// bundle 目录
        dir: PathBuf,
        /// 输出格式：json / toml
        #[arg(long, default_value = "json")]
        format: String,
        /// 最大导出深度
        #[arg(long)]
        depth: Option<usize>,
    },
    /// 平台适配：macOS 设置 bundle 位并让 `._meta` 可见
    Reveal {
        /// bundle 目录
        dir: PathBuf,
    },
    /// 列出全部错误码
    Codes,
}

#[derive(Subcommand)]
enum NodeCmd {
    /// 新增独立节点（深度 1）
    Add {
        /// bundle 目录
        dir: PathBuf,
        /// 实体类型，如 crm.customer
        #[arg(long = "type")]
        type_: Option<String>,
        /// 标题
        #[arg(long)]
        title: Option<String>,
        /// 摘要
        #[arg(long)]
        summary: Option<String>,
    },
}

#[derive(Subcommand)]
enum BranchCmd {
    /// 在指定分支下新增关联分支
    Add {
        /// bundle 目录
        dir: PathBuf,
        /// 锚点分支 id
        anchor: String,
        /// 类型
        #[arg(long = "type")]
        type_: Option<String>,
        /// 标题
        #[arg(long)]
        title: Option<String>,
        /// 摘要
        #[arg(long)]
        summary: Option<String>,
        /// 排序键
        #[arg(long)]
        order: Option<i64>,
    },
    /// 删除关联分支（含全部下级）
    Rm {
        /// bundle 目录
        dir: PathBuf,
        /// 分支 id
        uuid: String,
        /// 确认删除
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum RefCmd {
    /// 新增跨枝关联线
    Add {
        /// bundle 目录
        dir: PathBuf,
        /// 源分支 id
        uuid: String,
        /// 目标分支 id
        #[arg(long)]
        target: String,
        /// 关联语义
        #[arg(long, default_value = "related")]
        rel: String,
        /// 标签
        #[arg(long)]
        title: Option<String>,
        /// 备注
        #[arg(long)]
        note: Option<String>,
    },
    /// 删除关联线
    Rm {
        /// bundle 目录
        dir: PathBuf,
        /// 源分支 id
        uuid: String,
        /// 关联线 id
        #[arg(long = "ref")]
        ref_id: String,
    },
}

fn dispatch(cmd: Cmd) -> Result<i32> {
    match cmd {
        Cmd::Init {
            dir,
            name,
            title,
            summary,
        } => {
            cmd::init(&dir, name, title, summary)?;
            Ok(0)
        }
        Cmd::Validate {
            dir,
            strict,
            json,
            fix_manifest,
        } => cmd::validate(&dir, strict, json, fix_manifest),
        Cmd::Tree {
            dir,
            depth,
            show_refs,
        } => {
            cmd::tree(&dir, depth, show_refs)?;
            Ok(0)
        }
        Cmd::Ls { dir, uuid, raw } => {
            cmd::ls(&dir, uuid, raw)?;
            Ok(0)
        }
        Cmd::Show { dir, uuid, full } => {
            cmd::show(&dir, &uuid, full)?;
            Ok(0)
        }
        Cmd::Node { cmd } => match cmd {
            NodeCmd::Add {
                dir,
                type_,
                title,
                summary,
            } => {
                cmd::node_add(&dir, type_, title, summary)?;
                Ok(0)
            }
        },
        Cmd::Branch { cmd } => match cmd {
            BranchCmd::Add {
                dir,
                anchor,
                type_,
                title,
                summary,
                order,
            } => {
                cmd::branch_add(&dir, &anchor, type_, title, summary, order)?;
                Ok(0)
            }
            BranchCmd::Rm { dir, uuid, force } => {
                cmd::branch_rm(&dir, &uuid, force)?;
                Ok(0)
            }
        },
        Cmd::Ref { cmd } => match cmd {
            RefCmd::Add {
                dir,
                uuid,
                target,
                rel,
                title,
                note,
            } => {
                cmd::ref_add(&dir, &uuid, &target, rel, title, note)?;
                Ok(0)
            }
            RefCmd::Rm {
                dir,
                uuid,
                ref_id,
            } => {
                cmd::ref_rm(&dir, &uuid, &ref_id)?;
                Ok(0)
            }
        },
        Cmd::Sync { dir, dry_run } => {
            cmd::sync(&dir, dry_run)?;
            Ok(0)
        }
        Cmd::Fmt {
            dir,
            check,
            strip_comments,
        } => cmd::fmt(&dir, check, strip_comments),
        Cmd::Norm { dir, uuid } => {
            cmd::norm(&dir, uuid)?;
            Ok(0)
        }
        Cmd::Context {
            dir,
            uuid,
            depth,
            budget,
        } => {
            cmd::context(&dir, &uuid, depth, budget)?;
            Ok(0)
        }
        Cmd::Export {
            dir,
            format,
            depth,
        } => {
            cmd::export(&dir, format, depth)?;
            Ok(0)
        }
        Cmd::Reveal { dir } => {
            cmd::reveal(&dir)?;
            Ok(0)
        }
        Cmd::Codes => {
            cmd::list_codes();
            Ok(0)
        }
    }
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match dispatch(cli.cmd) {
        Ok(code) => std::process::ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("str: {e}");
            let code = match e {
                Error::NotFound(_) | Error::BadArg(_) => 2,
                Error::Io { .. } => 3,
                Error::Validation(_) | Error::Other(_) => 1,
            };
            std::process::ExitCode::from(code)
        }
    }
}
