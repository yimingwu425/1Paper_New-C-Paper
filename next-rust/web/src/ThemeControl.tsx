import {useEffect, useState} from 'react';

type Theme = 'system' | 'light' | 'dark';
const isTheme = (value: string | null): value is Theme => value === 'light' || value === 'dark' || value === 'system';

/** A local preference only. System mode follows OS changes through CSS. */
export function ThemeControl({storageKey}: {storageKey: string}) {
  const [theme, setTheme] = useState<Theme>(() => {
    try { const saved = localStorage.getItem(storageKey); return isTheme(saved) ? saved : 'system'; }
    catch { return 'system'; }
  });
  useEffect(() => {
    if (theme === 'system') document.documentElement.removeAttribute('data-theme');
    else document.documentElement.setAttribute('data-theme', theme);
  }, [theme]);
  function choose(value: string) {
    if (!isTheme(value)) return;
    setTheme(value);
    try { localStorage.setItem(storageKey, value); } catch { /* Appearance still works for this session. */ }
  }
  return <label className="im-theme-control"><span>外观</span><select aria-label="外观" value={theme} onChange={event => choose(event.target.value)}>
    <option value="system">跟随系统</option><option value="light">浅色</option><option value="dark">深色</option>
  </select></label>;
}
