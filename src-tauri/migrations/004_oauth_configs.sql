-- OAuth 配置表 - 支持多平台动态配置
BEGIN;

CREATE TABLE IF NOT EXISTS oauth_configs (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    provider        VARCHAR(32) NOT NULL UNIQUE
                            CHECK (provider IN ('github', 'google', 'microsoft', 'gitee', 'qq', 'wechat')),
    name            VARCHAR(64) NOT NULL,
    client_id       TEXT        NOT NULL,
    client_secret   TEXT        NOT NULL,
    redirect_uri    TEXT        NOT NULL,
    auth_url        TEXT        NOT NULL,
    token_url       TEXT        NOT NULL,
    userinfo_url    TEXT        NOT NULL,
    scopes          VARCHAR(256) NOT NULL DEFAULT 'user:email',
    enabled         BOOLEAN     NOT NULL DEFAULT FALSE,
    extra_config    JSONB       DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_oauth_configs_provider ON oauth_configs(provider);
CREATE INDEX IF NOT EXISTS idx_oauth_configs_enabled  ON oauth_configs(enabled) WHERE enabled = TRUE;

-- 初始化默认的 GitHub 配置（禁用状态）
INSERT INTO oauth_configs (provider, name, client_id, client_secret, redirect_uri, auth_url, token_url, userinfo_url, scopes, enabled)
VALUES (
    'github',
    'GitHub',
    'your_client_id',
    'your_client_secret',
    'http://localhost:9527/oauth/github/callback',
    'https://github.com/login/oauth/authorize',
    'https://github.com/login/oauth/access_token',
    'https://api.github.com/user',
    'user:email read:user',
    FALSE
) ON CONFLICT (provider) DO NOTHING;

COMMIT;
