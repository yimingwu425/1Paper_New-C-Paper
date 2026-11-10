// @vitest-environment jsdom
import {afterEach,beforeEach,describe,expect,it,vi} from 'vitest';
import {cleanup,fireEvent,render,screen} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import {ThemeControl} from './ThemeControl';

const storageKey='1mdesign.theme.test';
const appearance=()=>screen.getByRole('combobox',{name:'外观'}) as HTMLSelectElement;
beforeEach(()=>{localStorage.clear();document.documentElement.removeAttribute('data-theme')});
afterEach(()=>{cleanup();vi.restoreAllMocks();localStorage.clear();document.documentElement.removeAttribute('data-theme')});

describe('shared 1mDesign ThemeControl',()=>{
 it('falls back to system for an invalid saved preference',()=>{
  localStorage.setItem(storageKey,'unrecognized');
  document.documentElement.setAttribute('data-theme','dark');
  render(<ThemeControl storageKey={storageKey}/>);
  expect(appearance().value).toBe('system');
  expect(document.documentElement.hasAttribute('data-theme')).toBe(false);
 });
 it.each(['light','dark'])('restores and persists the explicit %s preference',theme=>{
  localStorage.setItem(storageKey,theme);
  const view=render(<ThemeControl storageKey={storageKey}/>);
  expect(appearance().value).toBe(theme);
  expect(document.documentElement.getAttribute('data-theme')).toBe(theme);
  const next=theme==='light'?'dark':'light';
  fireEvent.change(appearance(),{target:{value:next}});
  expect(localStorage.getItem(storageKey)).toBe(next);
  view.unmount();render(<ThemeControl storageKey={storageKey}/>);
  expect(appearance().value).toBe(next);
  expect(document.documentElement.getAttribute('data-theme')).toBe(next);
 });
 it('removes the explicit theme attribute when returning to system',()=>{
  localStorage.setItem(storageKey,'dark');
  render(<ThemeControl storageKey={storageKey}/>);
  fireEvent.change(appearance(),{target:{value:'system'}});
  expect(document.documentElement.hasAttribute('data-theme')).toBe(false);
  expect(localStorage.getItem(storageKey)).toBe('system');
 });
 it('keeps appearance usable when storage reads and writes are blocked',()=>{
  vi.spyOn(Storage.prototype,'getItem').mockImplementation(()=>{throw new DOMException('Blocked','SecurityError')});
  vi.spyOn(Storage.prototype,'setItem').mockImplementation(()=>{throw new DOMException('Blocked','SecurityError')});
  render(<ThemeControl storageKey={storageKey}/>);
  expect(appearance().value).toBe('system');
  fireEvent.change(appearance(),{target:{value:'dark'}});
  expect(appearance().value).toBe('dark');
  expect(document.documentElement.getAttribute('data-theme')).toBe('dark');
  fireEvent.change(appearance(),{target:{value:'system'}});
  expect(document.documentElement.hasAttribute('data-theme')).toBe(false);
 });
 it('exposes a labelled native select in keyboard tab order',async()=>{
  const user=userEvent.setup();
  render(<><ThemeControl storageKey={storageKey}/><button>Next control</button></>);
  const select=appearance();
  expect(select.tagName).toBe('SELECT');
  expect(screen.getByLabelText('外观')).toBe(select);
  expect(Array.from(select.options).map(option=>[option.value,option.text])).toEqual([
   ['system','跟随系统'],['light','浅色'],['dark','深色'],
  ]);
  await user.tab();expect(document.activeElement).toBe(select);
  // Native arrow-key selection is implemented by the browser, not jsdom.
  // Exercise the native change event and verify keyboard navigation remains intact.
  await user.selectOptions(select,'dark');
  expect(document.documentElement.getAttribute('data-theme')).toBe('dark');
  await user.tab();expect(document.activeElement).toBe(screen.getByRole('button',{name:'Next control'}));
  await user.tab({shift:true});expect(document.activeElement).toBe(select);
 });
});
