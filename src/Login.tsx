import { useState } from "react";
import { useI18n } from "./i18n";
import { loginWithToken } from "./platform";

interface LoginProps {
  onSuccess: () => void;
}

export default function Login({ onSuccess }: LoginProps) {
  const { t } = useI18n();
  const [token, setToken] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showPassword, setShowPassword] = useState(false);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = token.trim();
    if (!trimmed) {
      setError(t("login.tokenRequired"));
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const res = await loginWithToken(trimmed);
      if (res.ok) {
        onSuccess();
      } else {
        setError(res.error || t("login.invalidToken"));
      }
    } catch {
      setError(t("login.requestFailed"));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="login-container">
      <div className="login-card">
        <div className="login-header">
          <h1 className="wordmark-lg">Pipi</h1>
          <p className="login-desc">{t("login.description")}</p>
        </div>

        <form className="login-form" onSubmit={handleSubmit}>
          <div className="login-field">
            <label className="label" htmlFor="pipi-token-input">
              {t("login.tokenLabel")}
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
                title={showPassword ? t("login.hide") : t("login.show")}
                aria-label={showPassword ? t("login.hideToken") : t("login.showToken")}
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
            {loading ? t("login.verifying") : t("login.submit")}
          </button>
        </form>

        <div className="login-hint">
          <span>{t("login.hintBefore")}<code>PIPI_AUTH_TOKEN</code>{t("login.hintAfter")}</span>
        </div>
      </div>
    </div>
  );
}
