import { useConfirmStore } from '../stores/confirm';
import { AlertTriangle } from 'lucide-react';

/**
 * ConfirmDialog — 全局单例确认弹窗
 * 挂载在 App.tsx 根节点，整个应用共用一个实例。
 * 通过 useConfirm() hook 触发，返回 Promise<boolean>。
 */
export default function ConfirmDialog() {
  const { open, options, _accept, _cancel } = useConfirmStore();

  if (!open) return null;

  const {
    title = '确认操作',
    message,
    confirmText = '确认',
    cancelText = '取消',
    danger = false,
  } = options;

  return (
    <div
      className="modal-overlay"
      onClick={_cancel}
      style={{ zIndex: 1100 }}
    >
      <div
        className="modal"
        style={{ maxWidth: 420 }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* 图标 + 标题 */}
        <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 14 }}>
          <div style={{
            width: 36, height: 36,
            borderRadius: 10,
            background: danger ? 'rgba(248,113,113,0.12)' : 'rgba(124,106,247,0.12)',
            display: 'flex', alignItems: 'center', justifyContent: 'center',
            flexShrink: 0,
          }}>
            <AlertTriangle
              size={18}
              style={{ color: danger ? 'var(--danger)' : 'var(--accent)' }}
            />
          </div>
          <h2 style={{ fontSize: 16, fontWeight: 800, margin: 0 }}>{title}</h2>
        </div>

        {/* 消息正文 */}
        <p style={{
          fontSize: 13,
          color: 'var(--text-dim)',
          lineHeight: 1.7,
          marginBottom: 24,
        }}>
          {message}
        </p>

        {/* 操作按钮 */}
        <div className="modal-actions">
          <button className="btn btn-ghost" onClick={_cancel}>
            {cancelText}
          </button>
          <button
            className={`btn ${danger ? 'btn-danger' : 'btn-primary'}`}
            onClick={_accept}
            style={danger ? {
              background: 'rgba(248,113,113,0.15)',
              color: 'var(--danger)',
              border: '1px solid rgba(248,113,113,0.4)',
            } : undefined}
          >
            {confirmText}
          </button>
        </div>
      </div>
    </div>
  );
}

