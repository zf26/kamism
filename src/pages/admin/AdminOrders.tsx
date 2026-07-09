import { useEffect, useState } from 'react';
import { api } from '../../lib/api';
import { ChevronLeft, ChevronRight } from 'lucide-react';

interface Order {
  order_id: string;
  merchant_id: string;
  username: string;
  pay_channel: string;
  pay_type: string;
  amount: string;
  status: string;
  expires_days: number | null;
  created_at: string;
  pay_time: string | null;
  pay_price: string | null;
}

export default function AdminOrders() {
  const [orders, setOrders] = useState<Order[]>([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(1);
  const [loading, setLoading] = useState(true);
  const [statusFilter, setStatusFilter] = useState('');
  const [channelFilter, setChannelFilter] = useState('');
  const PAGE_SIZE = 20;

  const fetchOrders = async (p: number) => {
    setLoading(true);
    try {
      const params: Record<string, any> = { page: p, page_size: PAGE_SIZE };
      if (statusFilter) params.status = statusFilter;
      if (channelFilter) params.channel = channelFilter;
      const res = await api.get('/admin/payment/orders', { params });
      if (res.data.success) {
        setOrders(res.data.data);
        setTotal(res.data.total);
      }
    } catch {
      // silent
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { fetchOrders(1); }, []);
  useEffect(() => { setPage(1); fetchOrders(1); }, [statusFilter, channelFilter]);

  const totalPages = Math.ceil(total / PAGE_SIZE);

  const StatusTag = ({ status }: { status: string }) => {
    const map: Record<string, { label: string; color: string }> = {
      paid:      { label: '已支付', color: 'var(--success)' },
      pending:   { label: '待支付', color: 'var(--warning)' },
      expired:   { label: '已过期', color: 'var(--danger)' },
      cancelled: { label: '已取消', color: 'var(--text-muted)' },
      refunded:  { label: '已退款', color: 'var(--danger)' },
    };
    const m = map[status] || { label: status, color: 'var(--text-muted)' };
    return <span style={{ color: m.color, fontWeight: 600 }}>{m.label}</span>;
  };

  return (
    <div>
      <div className="page-header" style={{ marginBottom: 24 }}>
        <div>
          <h1 className="page-title">订单管理</h1>
          <p className="page-desc">查看所有支付订单</p>
        </div>
      </div>

      {/* 筛选栏 */}
      <div style={{ display: 'flex', gap: 12, marginBottom: 20, flexWrap: 'wrap' }}>
        <select
          value={statusFilter}
          onChange={e => setStatusFilter(e.target.value)}
          style={{ padding: '8px 12px', borderRadius: 8, border: '1px solid var(--border)', background: 'var(--bg-card)', color: 'var(--text)', fontSize: 13 }}
        >
          <option value="">全部状态</option>
          <option value="paid">已支付</option>
          <option value="pending">待支付</option>
          <option value="expired">已过期</option>
          <option value="cancelled">已取消</option>
          <option value="refunded">已退款</option>
        </select>
        <select
          value={channelFilter}
          onChange={e => setChannelFilter(e.target.value)}
          style={{ padding: '8px 12px', borderRadius: 8, border: '1px solid var(--border)', background: 'var(--bg-card)', color: 'var(--text)', fontSize: 13 }}
        >
          <option value="">全部渠道</option>
          <option value="alipay">支付宝</option>
          <option value="xorpay">XorPay</option>
          <option value="mbdpay">MbdPay</option>
        </select>
        <span style={{ fontSize: 13, color: 'var(--text-muted)', alignSelf: 'center' }}>
          共 {total} 条
        </span>
      </div>

      {/* 列表 */}
      <div className="card" style={{ padding: 0, overflowX: 'auto' }}>
        {loading ? (
          <div style={{ padding: 40, textAlign: 'center' }}><span className="spinner" /></div>
        ) : orders.length === 0 ? (
          <div style={{ padding: 40, textAlign: 'center', color: 'var(--text-muted)' }}>暂无订单</div>
        ) : (
          <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 13 }}>
            <thead>
              <tr style={{ borderBottom: '1px solid var(--border-light)' }}>
                {['订单号', '商户', '金额', '实付', '渠道', '状态', '天数', '创建时间', '支付时间'].map(h => (
                  <th key={h} style={{ padding: '12px 14px', textAlign: 'left', fontWeight: 600, color: 'var(--text-muted)', whiteSpace: 'nowrap' }}>{h}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {orders.map(o => (
                <tr key={o.order_id} style={{ borderBottom: '1px solid var(--border-light)' }}>
                  <td style={{ padding: '12px 14px', fontFamily: 'monospace', fontSize: 12, whiteSpace: 'nowrap' }}>{o.order_id.slice(-16)}</td>
                  <td style={{ padding: '12px 14px', whiteSpace: 'nowrap' }}>
                    <div style={{ fontWeight: 600 }}>{o.username}</div>
                    <div style={{ fontSize: 11, color: 'var(--text-muted)' }}>{o.merchant_id.slice(0, 8)}</div>
                  </td>
                  <td style={{ padding: '12px 14px', fontWeight: 700, whiteSpace: 'nowrap' }}>¥{o.amount}</td>
                  <td style={{ padding: '12px 14px', whiteSpace: 'nowrap' }}>
                    {o.pay_price ? <span style={{ color: 'var(--success)' }}>¥{o.pay_price}</span> : '—'}
                  </td>
                  <td style={{ padding: '12px 14px', whiteSpace: 'nowrap' }}>
                    {o.pay_channel === 'alipay' ? '支付宝' : o.pay_channel === 'xorpay' ? 'XorPay' : o.pay_channel === 'mbdpay' ? 'MbdPay' : o.pay_channel}
                  </td>
                  <td style={{ padding: '12px 14px', whiteSpace: 'nowrap' }}>
                    <StatusTag status={o.status} />
                  </td>
                  <td style={{ padding: '12px 14px', whiteSpace: 'nowrap', color: 'var(--text-muted)' }}>
                    {o.expires_days ? `${o.expires_days} 天` : '永久'}
                  </td>
                  <td style={{ padding: '12px 14px', whiteSpace: 'nowrap', color: 'var(--text-muted)', fontSize: 12 }}>
                    {new Date(o.created_at).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })}
                  </td>
                  <td style={{ padding: '12px 14px', whiteSpace: 'nowrap', color: 'var(--text-muted)', fontSize: 12 }}>
                    {o.pay_time ? new Date(o.pay_time).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }) : '—'}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {/* 分页 */}
      {totalPages > 1 && (
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 12, marginTop: 20 }}>
          <button className="btn btn-ghost" style={{ padding: '6px 14px', fontSize: 13 }}
            disabled={page <= 1}
            onClick={() => { const p = page - 1; setPage(p); fetchOrders(p); }}>
            <ChevronLeft size={14} /> 上一页
          </button>
          <span style={{ fontSize: 13, color: 'var(--text-muted)' }}>
            第 {page} / {totalPages} 页
          </span>
          <button className="btn btn-ghost" style={{ padding: '6px 14px', fontSize: 13 }}
            disabled={page >= totalPages}
            onClick={() => { const p = page + 1; setPage(p); fetchOrders(p); }}>
            下一页 <ChevronRight size={14} />
          </button>
        </div>
      )}
    </div>
  );
}
