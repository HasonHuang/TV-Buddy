//! 命令行子命令。
//!
//! 第一个参数是下列命令之一时，执行命令并退出。否则按数据库路径启动服务。
//!
//! | 命令 | 作用 |
//! | --- | --- |
//! | `config import <config.json>` | 从 JSON 文件导入整份配置 |
//! | `config get <a.b.c|.>` | 读一个配置值；`.` 表示整份配置 |
//! | `config set <a.b.c> <值>` | 写一个配置值 |
//! | `config reset` | 配置恢复默认，保留用户 |
//! | `passwd <用户名>` | 重设用户口令 |
//! | `update` | 从发布包替换当前二进制 |
//! | `version` | 显示版本 |
//!
//! 路径按 `.` 分段。纯数字段按数组下标，其余按对象键，例如
//! `groups.0.channels.1.url`。

use std::{
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::{
    config::{Config, ConfigStore},
    db,
    users::{password, UserStore},
};

/// 默认数据库文件。
const DEFAULT_DB: &str = "tv-buddy.db";

/// `config set` 遇到并发写入时的最大重试次数。含数组下标的路径不重试。
const CAS_RETRIES: usize = 3;

/// 参数解析结果。
struct Args {
    positional: Vec<String>,
    force: bool,
    stdin: bool,
    allow_extra: bool,
    reveal: bool,
    db: PathBuf,
}

impl Args {
    fn parse(raw: &[String]) -> Result<Self> {
        let mut parsed = Self {
            positional: Vec::new(),
            force: false,
            stdin: false,
            allow_extra: false,
            reveal: false,
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
                "--allow-extra" => parsed.allow_extra = true,
                "--reveal" => parsed.reveal = true,
                other => parsed.positional.push(other.to_string()),
            }
            index += 1;
        }
        Ok(parsed)
    }
}

/// 第一个参数是子命令时执行它。返回 `None` 表示按服务启动。
pub async fn run_if_subcommand() -> Result<Option<i32>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first() else {
        return Ok(None);
    };
    match command.as_str() {
        "config" => config_command(&args[1..])?,
        "passwd" | "pass" => passwd(&args[1..])?,
        "update" => crate::update::run(&args[1..]).await?,
        "version" | "--version" | "-V" => println!("tv-buddy {}", crate::VERSION),
        "--help" | "-h" | "help" => print_help(),
        _ => {
            // 裸词（如 `get`）无法与打错的子命令区分。若直接当数据库路径，
            // 会在当前目录生成 `<词>`、`<词>-wal` 与日志文件。只接受路径形式。
            if !looks_like_db_path(command) && !Path::new(command).is_file() {
                bail!(
                    "未知子命令：{command}\n\
                     若这是数据库路径，请写成 ./{command} 或使用 .db 后缀。\n\
                     子命令：config / passwd / update / version / help"
                );
            }
            return Ok(None);
        }
    }
    Ok(Some(0))
}

/// 判断第一个参数是否像数据库路径。
///
/// 服务启动时第一个位置参数是数据库路径。只接受绝对路径、含分隔符的路径、
/// 默认库名、点开头的路径或已知数据库后缀。裸词按未知子命令拒绝，
/// 避免误在当前目录建库。
fn looks_like_db_path(arg: &str) -> bool {
    if arg == DEFAULT_DB {
        return true;
    }
    let path = Path::new(arg);
    path.is_absolute()
        || arg.contains('/')
        || arg.contains('\\')
        || arg.starts_with('.')
        || matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("db" | "sqlite" | "sqlite3")
        )
}

fn print_help() {
    println!(
        "tv-buddy [数据库路径]\n\
         \x20 路径需含 / 或以 .db 结尾；否则按未知子命令拒绝\n\
         \n\
         子命令：\n\
         \x20 config import <config.json> [--db <路径>] [--force]    从文件导入整份配置\n\
         \x20 config get <a.b.c|.> [--db <路径>] [--reveal]            读配置值；默认隐藏密钥\n\
         \x20 config set <a.b.c> <值> [--db <路径>] [--allow-extra]  写配置值；新键默认告警\n\
         \x20 config reset [--db <路径>] [--force]                   配置恢复默认\n\
         \x20 pass|passwd <用户名> [--db <路径>] [--stdin]           重设口令\n\
         \x20 update [--check] [--version <tag>] [--repo <owner/name>]\n\
         \x20        [--archive <路径|URL>] [--restart] [--force]      更新二进制\n\
         \x20 version | --version                                   显示版本\n\
         \n\
         默认数据库：{DEFAULT_DB}"
    );
}

/// 提示：config 子命令只写库，运行中的服务不会自动读取。
fn print_reload_hint() {
    println!("提示：运行中的服务不会自动读取。请在面板点「重读配置」，或重启服务。");
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
            let old = store.load()?;
            let had_old = old.is_some();
            if !args.force && had_old {
                bail!("数据库已有配置。确需覆盖请加 --force");
            }
            let text =
                std::fs::read_to_string(source).with_context(|| format!("无法读取 {source}"))?;
            let mut config: Config = serde_json::from_str(&text).context("配置格式错误")?;
            let old = old.unwrap_or_default();
            crate::config::model::Config::mark_manual_mode_changes(&old, &mut config);
            crate::config::validate(&config).context("配置不合法")?;
            if had_old {
                backup_config(&old, &args.db)?;
            }
            match store.save_if_revision(&config, Some(old.revision)) {
                Ok((_, saved)) => {
                    println!("已导入配置：{} 个分组", saved.groups.len());
                    print_reload_hint();
                }
                Err(error)
                    if error
                        .downcast_ref::<crate::config::StaleRevision>()
                        .is_some() =>
                {
                    bail!("配置在导入期间被修改，请重试");
                }
                Err(error) => return Err(error),
            }
        }
        "get" => {
            let Some(path) = args.positional.get(1) else {
                bail!("用法：tv-buddy config get <a.b.c|.>");
            };
            let config = store.load()?.context("数据库里没有配置")?;
            let mut value = serde_json::to_value(&config)?;
            if !args.reveal {
                redact_secrets(&mut value);
            }
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
            let (_, _, existed) = set_config_with_retry(&store, path, &parse_value(text))?;
            if !existed && !args.allow_extra {
                eprintln!(
                    "警告：{path} 不是已知配置键，将存入 extra。拼写错误？加 --allow-extra 可关闭此警告。"
                );
            }
            println!("{path} = {text}");
            print_reload_hint();
        }
        "reset" => {
            let old = store.load()?;
            if !args.force && old.is_some() {
                bail!("配置已存在。确需恢复默认请加 --force");
            }
            if let Some(config) = &old {
                backup_config(config, &args.db)?;
            }
            let expected = old.as_ref().map(|config| config.revision).unwrap_or(0);
            match store.reset_if_revision(expected) {
                Ok(_) => {
                    println!("配置已恢复默认");
                    print_reload_hint();
                }
                Err(error)
                    if error
                        .downcast_ref::<crate::config::StaleRevision>()
                        .is_some() =>
                {
                    bail!("配置在重置期间被修改，请重试");
                }
                Err(error) => return Err(error),
            }
        }
        other => bail!("未知的 config 子命令: {other}"),
    }
    Ok(())
}

/// 写一个配置路径。用 `revision` 做条件保存。
///
/// 非数组路径遇到并发写入时重读重放。含数组下标的路径不重试，
/// 因为数组可能已被重排，重试会改到另一项。
/// 返回新版本、实际落库的配置，以及该路径原先是否存在。
fn set_config_with_retry(
    store: &ConfigStore,
    path: &str,
    value: &Value,
) -> Result<(u64, Config, bool)> {
    let has_index = path
        .split('.')
        .any(|part| !part.is_empty() && part.parse::<usize>().is_ok());
    let mut attempt = 0;
    loop {
        attempt += 1;
        let old = store.load()?.unwrap_or_default();
        let old_value = serde_json::to_value(&old)?;
        let existed = get_path(&old_value, path).is_some();
        let mut root = old_value;
        set_path(&mut root, path, value.clone())?;
        let mut config: Config = serde_json::from_value(root).context("配置格式错误")?;
        crate::config::model::Config::mark_manual_mode_changes(&old, &mut config);
        crate::config::validate(&config).context("配置不合法")?;
        match store.save_if_revision(&config, Some(old.revision)) {
            Ok((revision, saved)) => return Ok((revision, saved, existed)),
            Err(error)
                if error
                    .downcast_ref::<crate::config::StaleRevision>()
                    .is_some() =>
            {
                if has_index {
                    bail!("配置在写入期间被修改。路径含数组下标，请重读后再试");
                }
                if attempt >= CAS_RETRIES {
                    bail!("配置在写入期间被修改，请重试");
                }
            }
            Err(error) => return Err(error),
        }
    }
}

/// 覆盖整份配置前，把当前配置写到 `<库>.bak.json`。文件权限 0600。
fn backup_config(config: &Config, db: &Path) -> Result<()> {
    let mut name = db.as_os_str().to_os_string();
    name.push(".bak.json");
    let path = PathBuf::from(name);
    let text = serde_json::to_string_pretty(config)?;
    write_private(&path, &text).with_context(|| format!("写入配置备份失败: {}", path.display()))?;
    println!("已备份当前配置：{}", path.display());
    Ok(())
}

fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(text.as_bytes())?;
        drop(file);
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, text)?;
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
    let hash = password::hash(&password)?;
    if !users.set_password_hash(username, &hash)? {
        bail!("用户不存在: {username}");
    }
    println!("已更新用户 {username} 的口令");
    println!("运行中的服务在下次登录时读取新口令。重启服务会让旧会话失效。");
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
    read_hidden_line()
}

/// 从终端读一行，关闭回显。标准输入不是终端时按普通行读取。
#[cfg(unix)]
fn read_hidden_line() -> Result<String> {
    use std::io::IsTerminal;

    use rustix::termios::{tcgetattr, tcsetattr, LocalModes, OptionalActions};

    fn read_line_plain(stdin: &std::io::Stdin) -> Result<String> {
        let mut buffer = String::new();
        stdin.read_line(&mut buffer)?;
        Ok(buffer.trim_end_matches(['\n', '\r']).to_string())
    }

    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        return read_line_plain(&stdin);
    }

    let Ok(original) = tcgetattr(&stdin) else {
        return read_line_plain(&stdin);
    };
    let mut hidden = original.clone();
    hidden.local_modes.remove(LocalModes::ECHO);
    if tcsetattr(&stdin, OptionalActions::Now, &hidden).is_err() {
        return read_line_plain(&stdin);
    }

    // 读取失败或 panic 时也要恢复回显。
    struct Restore<'a> {
        stdin: &'a std::io::Stdin,
        modes: rustix::termios::Termios,
    }
    impl Drop for Restore<'_> {
        fn drop(&mut self) {
            let _ = rustix::termios::tcsetattr(
                self.stdin,
                rustix::termios::OptionalActions::Now,
                &self.modes,
            );
        }
    }
    let _restore = Restore {
        stdin: &stdin,
        modes: original,
    };

    let mut buffer = String::new();
    stdin.read_line(&mut buffer)?;
    eprintln!();
    Ok(buffer.trim_end_matches(['\n', '\r']).to_string())
}

#[cfg(not(unix))]
fn read_hidden_line() -> Result<String> {
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
        current = step(current, part)?;
    }
    Some(current)
}

/// 取一层。纯数字按数组下标，其余按对象键。
fn step<'a>(current: &'a Value, part: &str) -> Option<&'a Value> {
    match current {
        Value::Array(items) => items.get(part.parse::<usize>().ok()?),
        other => other.get(part),
    }
}

/// 写一层。纯数字按数组下标，其余按对象键。
///
/// 数组下标越界时报错，不自动扩展——静默补 `null` 会让后续反序列化失败。
fn set_path(root: &mut Value, path: &str, value: Value) -> Result<()> {
    let parts: Vec<&str> = path.split('.').filter(|part| !part.is_empty()).collect();
    let (last, parents) = parts.split_last().context("路径不能为空")?;
    let mut current = root;
    for part in parents {
        current = match current {
            Value::Array(items) => {
                let index: usize = part
                    .parse()
                    .with_context(|| format!("数组下标无效: {part}"))?;
                items
                    .get_mut(index)
                    .with_context(|| format!("数组下标越界: {index}"))?
            }
            Value::Object(map) => map.entry((*part).to_string()).or_insert_with(|| json!({})),
            _ => bail!("路径 {part} 之前的值不是对象或数组"),
        };
    }
    match current {
        Value::Array(items) => {
            let index: usize = last
                .parse()
                .with_context(|| format!("数组下标无效: {last}"))?;
            let slot = items
                .get_mut(index)
                .with_context(|| format!("数组下标越界: {index}"))?;
            *slot = value;
        }
        Value::Object(map) => {
            map.insert((*last).to_string(), value);
        }
        _ => bail!("路径 {last} 之前的值不是对象或数组"),
    }
    Ok(())
}

/// `config get` 默认隐藏密钥。`--reveal` 跳过。
fn redact_secrets(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                if is_secret_key(key) {
                    *item = Value::String("***".into());
                } else if key == "headers" {
                    redact_headers(item);
                } else if key == "m3u_header_lines" {
                    redact_header_lines(item);
                } else {
                    redact_secrets(item);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_secrets(item);
            }
        }
        _ => {}
    }
}

fn is_secret_key(key: &str) -> bool {
    matches!(
        key,
        "key" | "token" | "mtran_token" | "slice_sign_secret" | "password"
    )
}

/// 请求头里的认证值也不进配置输出。
fn redact_headers(value: &mut Value) {
    let Value::Array(items) = value else {
        return;
    };
    for item in items {
        let Value::Object(map) = item else {
            continue;
        };
        let name = map
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "authorization" | "proxy-authorization" | "cookie" | "set-cookie" | "x-token" | "token"
        ) {
            map.insert("value".into(), Value::String("***".into()));
        }
    }
}

/// `server.m3u_header_lines` 是原始行文本，按前缀识别认证头。
fn redact_header_lines(value: &mut Value) {
    let Value::Array(lines) = value else {
        return;
    };
    for line in lines {
        let Value::String(text) = line else {
            continue;
        };
        let lower = text.to_ascii_lowercase();
        if [
            "authorization:",
            "proxy-authorization:",
            "cookie:",
            "x-token:",
        ]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        {
            *line = Value::String("***".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_get_dotted_path() {
        let mut root = json!({ "server": { "bind": "[::]:9900" } });
        set_path(&mut root, "cache.limit_mb", json!(120)).expect("写入");
        assert_eq!(get_path(&root, "cache.limit_mb"), Some(&json!(120)));
        assert_eq!(get_path(&root, "server.bind"), Some(&json!("[::]:9900")));
    }

    #[test]
    fn dotted_path_walks_arrays() {
        let mut root = json!({ "groups": [ { "id": "hk", "channels": [ { "url": "a" } ] } ] });
        set_path(&mut root, "groups.0.channels.0.url", json!("b")).expect("写入");
        assert_eq!(
            get_path(&root, "groups.0.channels.0.url"),
            Some(&json!("b"))
        );
    }

    #[test]
    fn array_index_out_of_range_is_error() {
        let mut root = json!({ "groups": [] });
        assert!(set_path(&mut root, "groups.0.id", json!("x")).is_err());
    }

    #[test]
    fn value_parsing_prefers_json() {
        assert_eq!(parse_value("true"), json!(true));
        assert_eq!(parse_value("200"), json!(200));
        assert_eq!(parse_value("0.8"), json!(0.8));
        assert_eq!(parse_value("[::]:9900"), json!("[::]:9900"));
    }

    #[test]
    fn distinguishes_database_path_from_subcommand() {
        assert!(looks_like_db_path(DEFAULT_DB));
        assert!(looks_like_db_path("./tv-buddy.db"));
        assert!(looks_like_db_path("/opt/tv-buddy/tv-buddy.db"));
        assert!(looks_like_db_path("data/store"));
        assert!(looks_like_db_path("tv.sqlite3"));
        assert!(!looks_like_db_path("get"));
        assert!(!looks_like_db_path("set"));
        assert!(!looks_like_db_path("--db"));
    }

    #[test]
    fn redacts_secret_fields() {
        let mut root = json!({
            "server": {
                "bind": "127.0.0.1:9900",
                "m3u_header_lines": ["User-Agent: ok", "Authorization: Bearer x"]
            },
            "auth": {"slice_sign_secret": "s"},
            "share": {"key": "k"},
            "subtitle": {"translate": {"token": "t"}},
            "groups": [{
                "proxy": {"password": "p"},
                "headers": [
                    {"name": "Authorization", "value": "Bearer x"},
                    {"name": "Referer", "value": "r"}
                ]
            }]
        });
        redact_secrets(&mut root);

        assert_eq!(root["server"]["bind"], "127.0.0.1:9900");
        assert_eq!(root["server"]["m3u_header_lines"][0], "User-Agent: ok");
        assert_eq!(root["server"]["m3u_header_lines"][1], "***");
        assert_eq!(root["auth"]["slice_sign_secret"], "***");
        assert_eq!(root["share"]["key"], "***");
        assert_eq!(root["subtitle"]["translate"]["token"], "***");
        assert_eq!(root["groups"][0]["proxy"]["password"], "***");
        assert_eq!(root["groups"][0]["headers"][0]["value"], "***");
        assert_eq!(root["groups"][0]["headers"][1]["value"], "r");
    }
}
