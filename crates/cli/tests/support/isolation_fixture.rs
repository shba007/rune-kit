use std::{
    io::{self, BufRead, Write},
    time::Duration,
};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--child-read") {
        println!(
            "{}",
            if std::fs::read(&args[2]).is_ok() {
                "allowed"
            } else {
                "denied"
            }
        );
        return;
    }
    if args.get(1).map(String::as_str) == Some("--delayed-write") {
        std::thread::sleep(Duration::from_millis(1500));
        let _ = std::fs::write(&args[2], b"escaped child");
        return;
    }
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        let result = if let Some(path) = line.strip_prefix("read:") {
            if std::fs::read(path).is_ok() {
                "allowed"
            } else {
                "denied"
            }
            .to_owned()
        } else if let Some(path) = line.strip_prefix("write:") {
            if std::fs::write(path, b"changed").is_ok() {
                "allowed"
            } else {
                "denied"
            }
            .to_owned()
        } else if let Some(paths) = line.strip_prefix("rename:") {
            let (a, b) = paths.split_once('|').unwrap();
            if std::fs::rename(a, b).is_ok() {
                "allowed"
            } else {
                "denied"
            }
            .to_owned()
        } else if let Some(path) = line.strip_prefix("delete:") {
            if std::fs::remove_file(path).is_ok() {
                "allowed"
            } else {
                "denied"
            }
            .to_owned()
        } else if let Some(addr) = line.strip_prefix("network:") {
            println!("attempting-network");
            io::stdout().flush().unwrap();
            let addr = addr.parse().unwrap();
            if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok() {
                "allowed"
            } else {
                "denied"
            }
            .to_owned()
        } else if let Some(path) = line.strip_prefix("background:") {
            match std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--delayed-write", path])
                .spawn()
            {
                Ok(_) => "spawned".into(),
                Err(_) => "child-failed".into(),
            }
        } else if let Some(path) = line.strip_prefix("child:") {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--child-read", path])
                .output();
            match result {
                Ok(output) if output.status.success() => {
                    String::from_utf8_lossy(&output.stdout).trim().to_owned()
                }
                _ => "child-failed".into(),
            }
        } else if let Some(key) = line.strip_prefix("env:") {
            if std::env::var_os(key).is_some() {
                "present"
            } else {
                "absent"
            }
            .to_owned()
        } else {
            "unknown".into()
        };
        println!("{result}");
        io::stdout().flush().unwrap();
    }
}
