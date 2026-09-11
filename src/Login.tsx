import { useState } from "react";
import { loginWithToken } from "./platform";

interface LoginProps {
  onSuccess: () => void;
}

export default function Login({ onSuccess }: LoginProps) {
  const [token, setToken] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showPassword, setShowPassword] = useState(false);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = token.trim();
    if (!trimmed) {
      setError("请输入访问 Token");
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const res = await loginWithToken(trimmed);
      if (res.ok) {
        onSuccess();
      } else {
        setError(res.error || "Token 错误，请核对后重试");
      }
    } catch {
      setError("登录请求异常，请检查网络连接");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="login-container">
      <div className="login-card">
        <div className="login-header">
          <h1 className="wordmark-lg">Pipi</h1>
          <p className="login-desc">该服务已开启访问保护，请输入访问 Token 进入</p>
        </div>

        <form className="login-form" onSubmit={handleSubmit}>
          <div className="login-field">
            <label className="label" htmlFor="pipi-token-input">
              访问 Token
            </label>
            <div className="login-input-wrap">
              <input
                id="pipi-token-input"
                className="mono"
                type={showPassword ? "text" : "password"}
                value={token}
                onChange={(e) => {
                  setToken(e.target.value);
                  if (error) setError(null);
                }}
                placeholder="PIPI_AUTH_TOKEN"
                autoFocus
                autoComplete="current-password"
                disabled={loading}
              />
              <button
                type="button"
                className="login-toggle-pw"
                onClick={() => setShowPassword((prev) => !prev)}
                title={showPassword ? "隐藏" : "显示"}
                aria-label={showPassword ? "隐藏 Token" : "显示 Token"}
                aria-pressed={showPassword}
              >
                {showPassword ? "🙈" : "👁️"}
              </button>
            </div>
          </div>

          {error && (
            <div className="login-error" role="alert">
              <span>{error}</span>
            </div>
          )}

          <button
            type="submit"
            className="btn primary wide"
            disabled={loading || !token.trim()}
          >
            {loading ? "验证中…" : "进入 Pipi"}
          </button>
        </form>

        <div className="login-hint">
          <span>提示：服务端由环境变量 <code>PIPI_AUTH_TOKEN</code> 保护</span>
        </div>
      </div>
    </div>
  );
}
