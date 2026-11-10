import {useEffect, useRef, useState} from 'react';
import {Pause, Play} from 'lucide-react';
import type {Subject} from './api';

/** One real copy of each subject. Native scrolling keeps links clickable and focusable. */
export function SubjectFlow({subjects, filtered, onChoose}: {
  subjects: Subject[]; filtered: boolean; onChoose: (code: string) => void;
}) {
  const viewport = useRef<HTMLDivElement>(null);
  const interaction = useRef({pointer: false, focus: false});
  const [paused, setPaused] = useState(false);
  const [reduced, setReduced] = useState(() => window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false);
  useEffect(() => {
    const media = window.matchMedia?.('(prefers-reduced-motion: reduce)');
    if (!media) return;
    const update = () => setReduced(media.matches);
    update(); media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, []);
  useEffect(() => {
    const element = viewport.current;
    if (!element || reduced || paused || filtered || subjects.length < 2) return;
    let frame = 0, previous = 0, direction = 1, position = element.scrollLeft;
    const tick = (now: number) => {
      const elapsed = previous ? Math.min(now - previous, 80) : 0;
      previous = now;
      const max = Math.max(0, element.scrollWidth - element.clientWidth);
      if (!interaction.current.pointer && !interaction.current.focus && !document.hidden && max > 0) {
        // Ease gently into each turn instead of snapping back or cloning controls.
        const edge = Math.min(position, max - position);
        const speed = 10 + 12 * Math.min(1, Math.max(0, edge) / 80);
        position = Math.max(0, Math.min(max, position + direction * speed * elapsed / 1000));
        element.scrollLeft = position;
        if (position >= max) direction = -1;
        else if (position <= 0) direction = 1;
      } else position = element.scrollLeft;
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [filtered, paused, reduced, subjects.map(s => s.code).join(',')]);
  if (!subjects.length) return null;
  const moving = !filtered && !reduced && subjects.length > 1;
  return <div className={'subject-flow ' + (filtered ? 'filtered' : '')} data-motion={reduced ? 'reduced' : paused ? 'paused' : 'auto'}>
    <div className="subject-flow-viewport" ref={viewport}
      onPointerEnter={() => {interaction.current.pointer = true}}
      onPointerLeave={() => {interaction.current.pointer = false}}
      onPointerDown={e => {if (e.pointerType === 'touch' || e.pointerType === 'pen') setPaused(true)}}
      onWheel={() => setPaused(true)}
      onFocusCapture={() => {interaction.current.focus = true}}
      onBlurCapture={e => {if (!e.currentTarget.contains(e.relatedTarget as Node | null)) interaction.current.focus = false}}>
      <div className="subject-suggestions" role="group" aria-label="科目快捷选择">
        {subjects.map(s => <button type="button" key={s.code} aria-label={s.name+' '+s.code} onClick={() => onChoose(s.code)}><span className="subject-name">{s.name}</span><span className="subject-code">{s.code}</span></button>)}
      </div>
    </div>
    {moving && <button className="subject-flow-control" type="button" aria-label={paused ? '继续科目流动' : '暂停科目流动'} onClick={() => setPaused(value => !value)}>{paused ? <Play size={12}/> : <Pause size={12}/>}<span>{paused ? '继续流动' : '暂停流动'}</span></button>}
  </div>;
}
