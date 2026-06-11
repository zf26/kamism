-- OAuth 配置表 - 支持多平台动态配置
BEGIN;

-- OAuth 配置表
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

-- merchants 表新增 OAuth ID 字段
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name = 'merchants' AND column_name = 'github_id') THEN
        ALTER TABLE merchants ADD COLUMN github_id VARCHAR(64) UNIQUE;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name = 'merchants' AND column_name = 'google_id') THEN
        ALTER TABLE merchants ADD COLUMN google_id VARCHAR(64) UNIQUE;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name = 'merchants' AND column_name = 'microsoft_id') THEN
        ALTER TABLE merchants ADD COLUMN microsoft_id VARCHAR(64) UNIQUE;
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_merchants_github_id ON merchants(github_id) WHERE github_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_merchants_google_id ON merchants(google_id) WHERE google_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_merchants_microsoft_id ON merchants(microsoft_id) WHERE microsoft_id IS NOT NULL;

-- 初始化默认的 OAuth 配置
INSERT INTO oauth_configs (provider, name, client_id, client_secret, redirect_uri, auth_url, token_url, userinfo_url, scopes, enabled)
VALUES
    ('github', 'GitHub', 'your_client_id', 'your_client_secret', 'http://localhost:9527/oauth/github/callback',
     'https://github.com/login/oauth/authorize', 'https://github.com/login/oauth/access_token',
     'https://api.github.com/user', 'user:email read:user', FALSE),
    ('google', 'Google', 'your_client_id', 'your_client_secret', 'http://localhost:9527/oauth/google/callback',
     'https://accounts.google.com/o/oauth2/v2/auth', 'https://oauth2.googleapis.com/token',
     'https://www.googleapis.com/oauth2/v2/userinfo', 'email profile', FALSE),
    ('microsoft', 'Microsoft', 'your_client_id', 'your_client_secret', 'http://localhost:9527/oauth/microsoft/callback',
     'https://login.microsoftonline.com/common/oauth2/v2.0/authorize', 'https://login.microsoftonline.com/common/oauth2/v2.0/token',
     'https://graph.microsoft.com/oidc/userinfo', 'openid email profile', FALSE),
    ('qq', 'QQ', 'your_client_id', 'your_client_secret', 'http://localhost:9527/oauth/qq/callback',
     'https://graph.qq.com/oauth2.0/authorize', 'https://graph.qq.com/oauth2.0/token',
     'https://graph.qq.com/user/get_user_info', 'get_user_info', FALSE),
    ('wechat', 'WeChat', 'your_client_id', 'your_client_secret', 'http://localhost:9527/oauth/wechat/callback',
     'https://open.weixin.qq.com/connect/qrconnect', 'https://api.weixin.qq.com/sns/oauth2/access_token',
     'https://api.weixin.qq.com/sns/userinfo', 'snsapi_login', FALSE)
ON CONFLICT (provider) DO NOTHING;

COMMIT;
