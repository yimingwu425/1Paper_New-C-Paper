// @vitest-environment jsdom
import {afterEach, beforeEach, describe, expect, it, vi} from 'vitest';
import {act, cleanup, fireEvent, render, screen} from '@testing-library/react';
import {SubjectFlow} from './SubjectFlow';
const subjects = [{code:'9702',name:'Physics'},{code:'9709',name:'Mathematics'},{code:'9708',name:'Economics'}];
let callback: FrameRequestCallback, next = 0;
beforeEach(() => {
  vi.stubGlobal('requestAnimationFrame', vi.fn((fn: FrameRequestCallback) => {callback = fn; return ++next}));
  vi.stubGlobal('cancelAnimationFrame', vi.fn());
  vi.stubGlobal('matchMedia', vi.fn(() => ({matches:false, addEventListener:vi.fn(), removeEventListener:vi.fn()})));
});
afterEach(() => {cleanup(); vi.unstubAllGlobals()});
function setup() {
  const onChoose = vi.fn(); const view = render(<SubjectFlow subjects={subjects} filtered={false} onChoose={onChoose}/>);
  const viewport = view.container.querySelector('.subject-flow-viewport') as HTMLElement;
  Object.defineProperties(viewport, {scrollWidth:{value:1000},clientWidth:{value:400}});
  return {onChoose,view,viewport};
}
describe('flowing subject shortcuts', () => {
  it('renders only one clickable and keyboard-reachable copy of every subject', () => {
    const {onChoose} = setup();
    expect(screen.getAllByRole('button').length).toBe(4);
    fireEvent.click(screen.getByRole('button',{name:'Physics 9702'}));
    expect(onChoose).toHaveBeenCalledTimes(1); expect(onChoose).toHaveBeenCalledWith('9702');
  });
  it('moves gently using native scroll positions and pauses under pointer or focus', () => {
    const {viewport} = setup(); callback(100); callback(180);
    expect(viewport.scrollLeft).toBeGreaterThan(0);
    fireEvent.pointerEnter(viewport); const stopped = viewport.scrollLeft; callback(260);
    expect(viewport.scrollLeft).toBe(stopped);
    fireEvent.pointerLeave(viewport); fireEvent.focus(screen.getByRole('button',{name:'Physics 9702'})); callback(340);
    expect(viewport.scrollLeft).toBe(stopped);
    fireEvent.blur(screen.getByRole('button',{name:'Physics 9702'}),{relatedTarget:null}); callback(420);
    expect(viewport.scrollLeft).toBeGreaterThan(stopped);
  });
  it('offers an explicit persistent pause and resume control', () => {
    const {view} = setup(); fireEvent.click(screen.getByRole('button',{name:'暂停科目流动'}));
    expect(view.container.querySelector('[data-motion="paused"]')).toBeTruthy();
    expect(cancelAnimationFrame).toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button',{name:'继续科目流动'}));
    expect(view.container.querySelector('[data-motion="auto"]')).toBeTruthy();
  });
  it('keeps filtered results stationary and all controls available', () => {
    render(<SubjectFlow subjects={subjects} filtered onChoose={vi.fn()}/>);
    expect(requestAnimationFrame).not.toHaveBeenCalled();
    expect(screen.queryByRole('button',{name:'暂停科目流动'})).toBeNull();
    expect(screen.getAllByRole('button').length).toBe(3);
  });
  it('starts without animation for reduced motion and respects changes live', () => {
    let change: () => void = () => {};
    const media = {matches:true,addEventListener:vi.fn((_event: string,fn: () => void) => {change=fn}),removeEventListener:vi.fn()};
    vi.stubGlobal('matchMedia',vi.fn(() => media));
    const {view} = setup();
    expect(requestAnimationFrame).not.toHaveBeenCalled();
    expect(screen.queryByRole('button',{name:'暂停科目流动'})).toBeNull();
    expect(view.container.querySelector('[data-motion="reduced"]')).toBeTruthy();
    act(() => {media.matches=false; change()});
    expect(screen.getByRole('button',{name:'暂停科目流动'})).toBeTruthy();
    expect(requestAnimationFrame).toHaveBeenCalled();
    act(() => {media.matches=true; change()});
    expect(screen.queryByRole('button',{name:'暂停科目流动'})).toBeNull();
    expect(cancelAnimationFrame).toHaveBeenCalled();
    view.unmount(); expect(media.removeEventListener).toHaveBeenCalledWith('change',change);
  });
  it('stops automatic motion when manually scrolled and cleans up animation on unmount', () => {
    const {view,viewport} = setup(); fireEvent.wheel(viewport);
    expect(screen.getByRole('button',{name:'继续科目流动'})).toBeTruthy();
    view.unmount(); expect(cancelAnimationFrame).toHaveBeenCalled();
  });
  it.each(['touch','pen'])('keeps the row paused after a %s interaction', pointerType => {
    const {viewport} = setup();
    const event = new Event('pointerdown',{bubbles:true});
    Object.defineProperty(event,'pointerType',{value:pointerType}); fireEvent(viewport,event);
    expect(screen.getByRole('button',{name:'继续科目流动'})).toBeTruthy();
  });
  it('suspends movement while the document is hidden', () => {
    const {viewport} = setup();
    const hidden = vi.spyOn(document,'hidden','get').mockReturnValue(true);
    callback(100);callback(180);expect(viewport.scrollLeft).toBe(0);
    hidden.mockReturnValue(false);callback(260);expect(viewport.scrollLeft).toBeGreaterThan(0);
    hidden.mockRestore();
  });
  it('reverses smoothly at the end rather than jumping back to the beginning', () => {
    const {viewport} = setup(); viewport.scrollLeft=599;
    fireEvent.pointerEnter(viewport);callback(100);fireEvent.pointerLeave(viewport);
    callback(180);callback(260);expect(viewport.scrollLeft).toBe(600);
    callback(340);expect(viewport.scrollLeft).toBeLessThan(600);expect(viewport.scrollLeft).toBeGreaterThan(590);
  });
  it('splits a desktop catalogue into two rows with opposite motion and pauses both on interaction', () => {
    const catalogue=Array.from({length:12},(_,i)=>({code:String(9700+i),name:'Subject '+i}));
    const {container}=render(<SubjectFlow subjects={catalogue} filtered={false} onChoose={vi.fn()}/>);
    const rows=Array.from(container.querySelectorAll<HTMLElement>('.subject-flow-viewport'));
    expect(rows).toHaveLength(2);
    rows.forEach(row=>Object.defineProperties(row,{scrollWidth:{value:1000},clientWidth:{value:400}}));
    callback(100);expect(rows.map(row=>row.scrollLeft)).toEqual([0,600]);
    callback(180);expect(rows[0].scrollLeft).toBeGreaterThan(0);expect(rows[1].scrollLeft).toBeLessThan(600);
    const stopped=rows.map(row=>row.scrollLeft);fireEvent.pointerEnter(rows[0]);callback(260);
    expect(rows.map(row=>row.scrollLeft)).toEqual(stopped);
    const codes=Array.from(container.querySelectorAll<HTMLElement>('[data-subject-code]')).map(button=>button.dataset.subjectCode);
    expect(codes).toEqual(catalogue.map(subject=>subject.code));expect(new Set(codes).size).toBe(12);
  });
  it('uses three narrow-screen rows and preserves focused subjects when regrouping or reducing motion', () => {
    let narrowChange=()=>{},reducedChange=()=>{};
    const narrow={matches:false,addEventListener:vi.fn((_event:string,fn:()=>void)=>{narrowChange=fn}),removeEventListener:vi.fn()};
    const reduced={matches:false,addEventListener:vi.fn((_event:string,fn:()=>void)=>{reducedChange=fn}),removeEventListener:vi.fn()};
    vi.stubGlobal('matchMedia',vi.fn((query:string)=>query.includes('max-width')?narrow:reduced));
    const catalogue=Array.from({length:12},(_,i)=>({code:String(9700+i),name:'Subject '+i}));
    const {container}=render(<SubjectFlow subjects={catalogue} filtered={false} onChoose={vi.fn()}/>);
    const name='Subject 5 9705';act(()=>screen.getByRole('button',{name}).focus());
    act(()=>{narrow.matches=true;narrowChange()});
    expect(container.querySelectorAll('.subject-flow-viewport')).toHaveLength(3);
    expect(document.activeElement).toBe(screen.getByRole('button',{name}));
    act(()=>{reduced.matches=true;reducedChange()});
    expect(container.querySelectorAll('.subject-flow-viewport')).toHaveLength(1);
    expect(document.activeElement).toBe(screen.getByRole('button',{name}));
    expect(screen.getAllByRole('button')).toHaveLength(12);
    expect(container.querySelector('[data-motion="reduced"]')).toBeTruthy();
  });
});
