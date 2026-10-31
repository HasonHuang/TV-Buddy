//! 命令行子命令。
//!
//! 第一个参数是下列命令之一时，执行命令并退出。否则按数据库路径启动服务。
//!
//! | 命令 | 作用 |
//! | --- | --- |
//! | `config import <config.json>` | 从 JSON 文件导入整份配置 |
//! | `config get <a.b.c>` | 读一个配置值 |
//! | `config set <a.b.c> <值>` | 写一个配置值 |
//! | `config reset` | 配置恢复默认，保留用户 |
//! | `passwd <用户名>` | 重设用户口令 |

use std::{io::Read, path::PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::{
    config::{Config, ConfigStore},
    db,
    users::{password, UserStore},
};

/// 默认数据库文件。
const DEFAULT_DB: &str = "tv-buddy.db";

/// 参数解析结果。
struct Args {
    positional: Vec<String>,
    force: bool,
    stdin: bool,
    db: PathBuf,
}

impl Args {
    fn parse(raw: &[String]) -> Result<Self> {
        let mut parsed = Self {
            positional: Vec::new(),
            force: false,
            stdin: false,
            db: PathBuf::from(DEFAULT_DB),
        };
        let mut index = 0;
        while index < raw.len() {
            match raw[index].as_str() {
                "--db" => {
                    index += 1;
                    let Some(value) = raw.get(index) else {
                        bail!("--db 缺少路径");
                    };
                    parsed.db = PathBuf::from(value);
                }
                "--force" => parsed.force = true,
                "--stdin" => parsed.stdin = true,
                other => parsed.positional.push(other.to_string()),
            }
            index += 1;
        }
        Ok(parsed)
    }
}

/// 第一个参数是子命令时执行它。返回 `None` 表示按服务启动。
pub fn run_if_subcommand() -> Result<Option<i32>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first() else {
        return Ok(None);
    };
    match command.as_str() {
        "config" => config_command(&args[1..])?,
        "passwd" => passwd(&args[1..])?,
        "--help" | "-h" | "help" => print_help(),
        _ => return Ok(None),
    }
    Ok(Some(0))
}

fn print_help() {
    println!(
        "tv-buddy [数据库路径]\n\
         \n\
         子命令：\n\
         \x20 config import <config.json> [--db <路径>] [--force]    从文件导入整份配置\n\
         \x20 config get <a.b.c> [--db <路径>]                       读配置值\n\
         \x20 config set <a.b.c> <值> [--db <路径>]                  写配置值\n\
         \x20 config reset [--db <路径>] [--force]                   配置恢复默认\n\
         \x20 passwd <用户名> [--db <路径>] [--stdin]                重设口令\n\
         \n\
         默认数据库：{DEFAULT_DB}"
    );
}

fn config_command(raw: &[String]) -> Result<()> {
    let args = Args::parse(raw)?;
    let Some(action) = args.positional.first().map(String::as_str) else {
        bail!("用法：tv-buddy config import|get|set|reset ...");
    };
    let conn = db::connect(&args.db)?;
    let store = ConfigStore::new(conn);

    match action {
        "import" => {
            let Some(source) = args.positional.get(1) else {
                bail!("用法：tv-buddy config import <config.json>");
            };
            if !args.force && store.load()?.is_some() {
                bail!("数据库已有配置。确需覆盖请加 --force");
            }
            let text =
                std::fs::read_to_string(source).with_context(|| format!("无法读取 {source}"))?;
            let config: Config = serde_json::from_str(&text).context("配置格式错误")?;
            store.save(&config)?;
            println!("已导入配置：{} 个分组", config.groups.len());
        }
        "get" => {
            let Some(path) = args.positional.get(1) else {
                bail!("用法：tv-buddy config get <a.b.c>");
            };
            let config = store.load()?.context("数据库里没有配置")?;
            let value = serde_json::to_value(&config)?;
            let found = get_path(&value, path).with_context(|| format!("配置项不存在: {path}"))?;
            match found {
                Value::String(text) => println!("{text}"),
                other => println!("{other}"),
            }
        }
        "set" => {
            let (Some(path), Some(text)) = (args.positional.get(1), args.positional.get(2)) else {
                bail!("用法：tv-buddy config set <a.b.c> <值>");
            };
            let mut value = serde_json::to_value(store.load()?.unwrap_or_default())?;
            set_path(&mut value, path, parse_value(text))?;
            let config: Config = serde_json::from_value(value).context("配置格式错误")?;
            store.save(&config)?;
            println!("{path} = {text}");
        }
        "reset" => {
            if !args.force && store.load()?.is_some() {
                bail!("配置已存在。确需恢复默认请加 --force");
            }
            store.reset()?;
            println!("配置已恢复默认");
        }
        other => bail!("未知的 config 子命令: {other}"),
    }
    Ok(())
}

fn passwd(raw: &[String]) -> Result<()> {
    let args = Args::parse(raw)?;
    let Some(username) = args.positional.first() else {
        bail!("用法：tv-buddy passwd <用户名> [--db <路径>] [--stdin]");
    };
    let password = read_password(args.stdin)?;
    if password.chars().count() < 4 {
        bail!("密码太短（至少 4 位）");
    }

    let conn = db::connect(&args.db)?;
    let users = UserStore::attach(conn)?;
    let user = users
        .find_by_username(username)
        .with_context(|| format!("用户不存在: {username}"))?;
    let hash = password::hash(&password)?;
    users.update_user(crate::users::User {
        password_hash: hash,
        ..user
    })?;
    println!("已更新用户 {username} 的口令");
    Ok(())
}

/// 读口令。`--stdin` 时从标准输入整段读取，去掉行尾换行。
fn read_password(from_stdin: bool) -> Result<String> {
    if from_stdin {
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer)?;
        return Ok(buffer.trim_end_matches(['\n', '\r']).to_string());
    }
    eprintln!("新密码：");
    let mut buffer = String::new();
    std::io::stdin().read_line(&mut buffer)?;
    Ok(buffer.trim_end_matches(['\n', '\r']).to_string())
}

/// 值按 JSON 解析。解析不了就当字符串。
fn parse_value(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
}

fn get_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = root;
    for part in path.split('.').filter(|part| !part.is_empty()) {
        current = current.get(part)?;
    }
    Some(current)
}

fn set_path(root: &mut Value, path: &str, value: Value) -> Result<()> {
    let parts: Vec<&str> = path.split('.').filter(|part| !part.is_empty()).collect();
    let (last, parents) = parts.split_last().context("路径不能为空")?;
    let mut current = root;
    for part in parents {
        let map = current.as_object_mut().context("路径指向的不是对象")?;
        current = map.entry((*part).to_string()).or_insert_with(|| json!({}));
    }
    let map = current.as_object_mut().context("路径指向的不是对象")?;
    map.insert((*last).to_string(), value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_get_dotted_path() {
        let mut root = json!({ "server": { "bind": "[::]:19899" } });
        set_path(&mut root, "cache.gc.max_disk_mb", json!(120)).expect("写入");
        assert_eq!(get_path(&root, "cache.gc.max_disk_mb"), Some(&json!(120)));
        assert_eq!(get_path(&root, "server.bind"), Some(&json!("[::]:19899")));
    }

    #[test]
    fn value_parsing_prefers_json() {
        assert_eq!(parse_value("true"), json!(true));
        assert_eq!(parse_value("200"), json!(200));
        assert_eq!(parse_value("0.8"), json!(0.8));
        assert_eq!(parse_value("[::]:19899"), json!("[::]:19899"));
    }
}
