import { useEffect, useState } from 'react';

/**
 * useDebouncedValue — 输入防抖。
 *
 * 列表页的搜索框若直接把 `value` 塞进 useEffect 依赖，每敲一个字符就发一次请求，
 * 既打后端、又制造竞态。这个 hook 把「即时输入值」延迟 delay 毫秒后才真正生效，
 * 只有稳定下来（停止输入）才触发一次查询。
 *
 * 用法：
 *   const [searchCode, setSearchCode] = useState('');
 *   const debouncedSearch = useDebouncedValue(searchCode, 300);
 *   useEffect(() => { load(page, pageSize, debouncedSearch); }, [page, pageSize, debouncedSearch]);
 */
export function useDebouncedValue<T>(value: T, delay = 300): T {
  const [debounced, setDebounced] = useState(value);

  useEffect(() => {
    const id = setTimeout(() => setDebounced(value), delay);
    return () => clearTimeout(id);
  }, [value, delay]);

  return debounced;
}
