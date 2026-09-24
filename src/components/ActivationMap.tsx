import { useEffect, useRef } from 'react';
import * as echarts from 'echarts';
import { useThemeStore } from '../stores/theme';

export interface IpDistributionItem {
  name: string;
  value: number;
}

/**
 * 卡密激活地图：按省级行政区着色显示客户 IP 分布。
 *
 * 合规说明（重要）：
 *   - 地图边界数据来自阿里 DataV（`geo.datav.aliyun.com/areas_v3/bound/100000_full.json`），
 *     是国产数据可视化事实标准，含台湾省、港澳、南海诸岛（十段线）的完整审图边界，
 *     不涉及境外瓦片底图（Google/OSM 等），符合中国地图合规要求。
 *   - 坐标是 GeoJSON 原始坐标系（非 GCJ-02），用于「专题统计着色」而非「导航定位」，
 *     所以不涉及坐标系纠偏问题。
 *
 * 数据语义：
 *   - 后端按「省」聚合（国外 IP 落到「国家」），`name` 对应 GeoJSON 的省级行政区名；
 *   - 只统计有 IP 归属地的激活，解析不到的内网/未收录 IP 不计入地图。
 */
export default function ActivationMap({ data }: { data: IpDistributionItem[] }) {
  const ref = useRef<HTMLDivElement>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);
  const { theme } = useThemeStore();
  const isDark = theme === 'dark';

  useEffect(() => {
    const el = ref.current;
    if (!el) return;

    if (!chartRef.current) {
      chartRef.current = echarts.init(el);
    }
    const chart = chartRef.current;

    // 竞态防护：data/theme 快速变化会触发 effect 重新执行 + cleanup 会 dispose 旧 chart，
    // 而 fetch GeoJSON 是异步的 —— 若旧请求返回后仍 render 到已 dispose 的实例会报错。
    // 用 cancelled 标志让「已作废的那一次 effect」的后续回调全部短路。
    let cancelled = false;

    // 加载中国地图 GeoJSON（放在 public/，Vite 直接 serve）
    fetch('/china-map.json')
      .then(r => {
        if (!r.ok) throw new Error(`加载地图数据失败: ${r.status}`);
        return r.json();
      })
      .then(geoJson => {
        if (cancelled) return;
        echarts.registerMap('china', geoJson);
        // 只把「境内」数据喂给地图（「境外」在组件里用图例文字单独展示）
        render(chart, data.filter(d => d.name !== '境外'), isDark);
      })
      .catch(e => {
        if (cancelled) return;
        console.error('[激活地图] 加载 GeoJSON 失败', e);
        // 加载失败不能静默白屏：显示错误占位
        chart.setOption({
          title: {
            text: '地图数据加载失败',
            left: 'center',
            top: 'middle',
            textStyle: { color: isDark ? '#888899' : '#4a4a5a', fontSize: 13, fontWeight: 'normal' },
          },
        });
      });

    const onResize = () => chart.resize();
    window.addEventListener('resize', onResize);
    return () => {
      cancelled = true;
      window.removeEventListener('resize', onResize);
      chart.dispose();
      chartRef.current = null;
    };
  }, [data, isDark]);

  // 境外数据单独剥离：地图边界只有中国省级行政区，「境外」桶不在 GeoJSON 里，
  // 硬塞进 map series 会被 echarts 静默丢弃（name 不匹配）。单独展示更诚实。
  const abroad = data.filter(d => d.name === '境外');
  const domestic = data.filter(d => d.name !== '境外');

  if (domestic.length === 0 && abroad.length === 0) {
    return (
      <div className="empty-state" style={{ padding: '40px 0' }}>
        <div className="empty-state-icon">🗺️</div>
        <div className="empty-state-text">暂无客户 IP 分布数据</div>
      </div>
    );
  }

  return (
    <div>
      <div ref={ref} style={{ width: '100%', height: 480 }} />
      {abroad.length > 0 && (
        <div
          style={{
            marginTop: 8,
            fontSize: 12,
            color: 'var(--text-dim)',
            display: 'flex',
            alignItems: 'center',
            gap: 6,
            justifyContent: 'flex-end',
          }}
        >
          <span style={{ color: 'var(--text-muted)' }}>境外激活：</span>
          <span style={{ fontWeight: 600, color: 'var(--text)' }}>
            {abroad.reduce((s, a) => s + a.value, 0)} 次
          </span>
        </div>
      )}
    </div>
  );
}

function render(chart: echarts.ECharts, data: IpDistributionItem[], isDark: boolean) {
  const max = Math.max(1, ...data.map(d => d.value));
  const textColor = isDark ? '#e8e8f0' : '#18181f';
  const subColor = isDark ? '#55556a' : '#8888a0';
  const borderColor = isDark ? '#2a2a3e' : '#c8c8da';

  chart.setOption({
    tooltip: {
      trigger: 'item',
      backgroundColor: isDark ? '#111118' : '#ffffff',
      borderColor,
      textStyle: { color: textColor, fontSize: 12 },
      formatter: (p: any) => {
        const v = p.value ?? 0;
        return `${p.name}<br/>激活次数：${v}`;
      },
    },
    visualMap: {
      min: 0,
      max,
      left: 10,
      bottom: 10,
      text: ['高', '低'],
      calculable: true,
      inRange: {
        // 激活越多越红（与项目「涨红跌绿」无冲突，这里是热度语义）
        color: isDark
          ? ['#1e1e2e', '#5a3a7a', '#7c6af7', '#e05a8a', '#ff6b6b']
          : ['#f4f4f8', '#c9c0f5', '#7c6af7', '#d94f7f', '#e0315f'],
      },
      textStyle: { color: subColor, fontSize: 10 },
    },
    series: [
      {
        name: '激活分布',
        type: 'map',
        map: 'china',
        roam: true,
        scaleLimit: { min: 0.8, max: 3 },
        label: {
          show: false,
        },
        emphasis: {
          label: { show: true, color: textColor, fontSize: 11 },
          itemStyle: { areaColor: isDark ? '#2a2a3e' : '#eeeef5' },
        },
        itemStyle: {
          borderColor,
          borderWidth: 0.5,
          areaColor: isDark ? '#16161f' : '#f4f4f8',
        },
        data: data.map(d => ({ name: d.name, value: d.value })),
      },
    ],
  });
}
