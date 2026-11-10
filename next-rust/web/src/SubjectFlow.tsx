import {useEffect, useRef, useState} from 'react';
import {Pause, Play} from 'lucide-react';
import type {Subject} from './api';

/** One real copy of each subject. Native scrolling keeps links clickable and focusable. */
export function SubjectFlow({subjects, filtered, onChoose}: {
  subjects: Subject[]; filtered: boolean; onChoose: (code: string) => void;
}) {
  const lanes = useRef<HTMLDivElement>(null);
  const viewports = useRef<(HTMLDivElement | null)[]>([]);
  const motion = useRef(new WeakMap<HTMLElement,{position:number;direction:number}>());
  const restoreFocus = useRef<string | undefined>(undefined);
  const interaction = useRef({pointer: false, focus: false});
  const [paused, setPaused] = useState(false);
  const [reduced, setReduced] = useState(() => window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false);
  const [narrow, setNarrow] = useState(() => window.matchMedia?.('(max-width: 650px)').matches ?? false);
  const rowCount = filtered || reduced ? 1 : Math.min(narrow ? 3 : 2, Math.max(1, Math.ceil(subjects.length / 3)));
  const rowSize = Math.ceil(subjects.length / rowCount);
  const rows = Array.from({length:rowCount}, (_,index) => subjects.slice(index * rowSize, (index + 1) * rowSize));
  useEffect(() => {
    const media = window.matchMedia?.('(max-width: 650px)');
    if (!media) return;
    const update = () => {
      restoreFocus.current = lanes.current?.querySelector<HTMLButtonElement>('button:focus')?.dataset.subjectCode;
      setNarrow(media.matches);
    };
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, []);
  useEffect(() => {
    if (!restoreFocus.current) return;
    Array.from(lanes.current?.querySelectorAll<HTMLButtonElement>('[data-subject-code]') ?? []).find(button => button.dataset.subjectCode === restoreFocus.current)?.focus();
    restoreFocus.current = undefined;
  }, [rowCount]);
  useEffect(() => {
    const media = window.matchMedia?.('(prefers-reduced-motion: reduce)');
    if (!media) return;
    const update = () => {
      restoreFocus.current = lanes.current?.querySelector<HTMLButtonElement>('button:focus')?.dataset.subjectCode;
      setReduced(media.matches);
    };
    update(); media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, []);
  useEffect(() => {
    if (reduced || paused || filtered || subjects.length < 2) return;
    const elements = viewports.current.slice(0, rowCount).filter((element): element is HTMLDivElement => !!element);
    let frame = 0, previous = 0;
    const tick = (now: number) => {
      const elapsed = previous ? Math.min(now - previous, 80) : 0;
      previous = now;
      elements.forEach((element, index) => {
        const max = Math.max(0, element.scrollWidth - element.clientWidth);
        let state = motion.current.get(element);
        if (!state) {
          state = {position:index % 2 ? max : element.scrollLeft,direction:index % 2 ? -1 : 1};
          motion.current.set(element,state);
          if (!interaction.current.pointer && !interaction.current.focus) element.scrollLeft = state.position;
        }
        if (!interaction.current.pointer && !interaction.current.focus && !document.hidden && max > 0) {
        // Ease gently into each turn instead of snapping back or cloning controls.
          const edge = Math.min(state.position, max - state.position);
          const speed = (10 + 12 * Math.min(1, Math.max(0, edge) / 80)) * (index === 1 ? .82 : index === 2 ? .94 : 1);
          state.position = Math.max(0, Math.min(max, state.position + state.direction * speed * elapsed / 1000));
          element.scrollLeft = state.position;
          if (state.position >= max) state.direction = -1;
          else if (state.position <= 0) state.direction = 1;
        } else state.position = element.scrollLeft;
      });
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [filtered, paused, reduced, rowCount, subjects.map(s => s.code).join(',')]);
  if (!subjects.length) return null;
  const moving = !filtered && !reduced && subjects.length > 1;
  return <div className={'subject-flow ' + (filtered ? 'filtered' : '')} data-motion={reduced ? 'reduced' : paused ? 'paused' : 'auto'}>
    <div className="subject-flow-lanes" ref={lanes} role="group" aria-label="科目快捷选择" data-rows={rowCount}
      onPointerEnter={() => {interaction.current.pointer = true}}
      onPointerLeave={() => {interaction.current.pointer = false}}
      onPointerDown={e => {if (e.pointerType === 'touch' || e.pointerType === 'pen') setPaused(true)}}
      onWheel={() => setPaused(true)}
      onFocusCapture={() => {interaction.current.focus = true}}
      onBlurCapture={e => {if (!e.currentTarget.contains(e.relatedTarget as Node | null)) interaction.current.focus = false}}>
      {rows.map((row,index) => <div className="subject-flow-viewport" key={index} ref={element => {viewports.current[index] = element}}>
        <div className="subject-suggestions">
          {row.map(s => <button type="button" key={s.code} data-subject-code={s.code} aria-label={s.name+' '+s.code} onClick={() => onChoose(s.code)}><span className="subject-name">{s.name}</span><span className="subject-code">{s.code}</span></button>)}
        </div>
      </div>)}
    </div>
    {moving && <button className="subject-flow-control" type="button" aria-label={paused ? '继续科目流动' : '暂停科目流动'} onClick={() => setPaused(value => !value)}>{paused ? <Play size={12}/> : <Pause size={12}/>}<span>{paused ? '继续流动' : '暂停流动'}</span></button>}
  </div>;
}
