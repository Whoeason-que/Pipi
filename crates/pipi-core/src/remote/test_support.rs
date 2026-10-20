use std::{fs, os::unix::fs::PermissionsExt};

/// 调用方必须持有 HOME_LOCK；也覆盖 PATH，使测试不会连接真实 Cloudflare。
pub(super) struct Environment(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl Environment {
    pub fn new(root: &std::path::Path) -> Self {
        let mut previous = Vec::new();
        for (key, path) in [
            ("PATH", root.to_owned()),
            ("PIPI_HOME", root.join("data")),
            ("XDG_CONFIG_HOME", root.join("config")),
        ] {
            previous.push((key, std::env::var_os(key)));
            pipi_test_support::set_env_var(key, path);
        }
        Self(previous)
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        for (key, previous) in self.0.drain(..).rev() {
            if let Some(value) = previous {
                pipi_test_support::set_env_var(key, value);
            } else {
                pipi_test_support::remove_env_var(key);
            }
        }
    }
}

pub(super) fn tunnel_fixture() -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let root = pipi_test_support::temp_dir("pipi-tunnel");
    let token = root.join("credential");
    fs::write(&token, "fixture-secret-never-in-argv").unwrap();
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    let binary = root.join("cloudflared");
    fs::write(
        &binary,
        r#"#!/usr/bin/python3
import http.server, os, pathlib, sys
root = pathlib.Path(__file__).parent
args = sys.argv[1:]
assert args[-2:] == ['--token-file', str(root / 'credential')]
assert '--no-autoupdate' in args
assert not any(key.startswith(('TUNNEL_', 'CF_')) for key in os.environ)
assert 'fixture-secret-never-in-argv' not in str(args)
(root / 'pid').write_text(str(os.getpid()))
if (root / 'exit').exists(): sys.exit(23)
host, port = args[args.index('--metrics') + 1].rsplit(':', 1)
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        assert self.path == '/ready'
        self.send_response(200 if (root / 'connected').exists() else 503)
        self.end_headers()
    def log_message(self, *args): pass
http.server.HTTPServer((host, int(port)), Handler).serve_forever()
"#,
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    (root, binary, token)
}
