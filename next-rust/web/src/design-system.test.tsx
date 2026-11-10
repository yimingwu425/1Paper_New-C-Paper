// @vitest-environment jsdom
import {afterEach,beforeEach,describe,it,expect,vi} from 'vitest';
import {cleanup,fireEvent,render,screen,waitFor} from '@testing-library/react';
import {ThemeControl} from './ThemeControl';
import {App} from './App';
const key='cpaper.1mdesign.theme';
beforeEach(()=>{localStorage.clear();document.documentElement.removeAttribute('data-theme');vi.stubGlobal('fetch',vi.fn(async (url:RequestInfo|URL)=>new Response(JSON.stringify(String(url)==='/api/favorites'?{favorites:[]}:String(url).includes('/catalog/subjects')?{subjects:[]}:{status:'ok'}),{headers:{'content-type':'application/json'}})))});
afterEach(()=>{cleanup();vi.unstubAllGlobals();localStorage.clear();document.documentElement.removeAttribute('data-theme')});
describe('1mDesign appearance integration',()=>{
 it('keeps C-Paper identity, original icon and accessible routes',async()=>{
  render(<App/>);
  const brand=screen.getByRole('button',{name:'C-Paper'});
  expect(brand.querySelector('img')?.getAttribute('src')).toBe('/icons/v2/app-icon.png');
  expect(screen.getByRole('button',{name:'找卷'}).getAttribute('aria-current')).toBe('page');
  fireEvent.click(screen.getByRole('button',{name:'批量下载'}));
  expect(screen.getByRole('heading',{name:'批量下载'})).toBeTruthy();
  expect(screen.getByRole('button',{name:'批量下载'}).getAttribute('aria-current')).toBe('page');
  expect(screen.getByRole('combobox',{name:'外观'})).toBeTruthy();
 });
 it('persists explicit themes and restores the system preference',()=>{
  const view=render(<ThemeControl storageKey={key}/>);
  fireEvent.change(screen.getByRole('combobox',{name:'外观'}),{target:{value:'dark'}});
  expect(document.documentElement.getAttribute('data-theme')).toBe('dark');
  expect(localStorage.getItem(key)).toBe('dark');
  view.unmount();render(<ThemeControl storageKey={key}/>);
  expect((screen.getByRole('combobox',{name:'外观'}) as HTMLSelectElement).value).toBe('dark');
  fireEvent.change(screen.getByRole('combobox',{name:'外观'}),{target:{value:'light'}});
  expect(document.documentElement.getAttribute('data-theme')).toBe('light');
  fireEvent.change(screen.getByRole('combobox',{name:'外观'}),{target:{value:'system'}});
  expect(document.documentElement.hasAttribute('data-theme')).toBe(false);
 });
 it('ignores invalid persisted appearance values',()=>{
  localStorage.setItem(key,'invalid');render(<ThemeControl storageKey={key}/>);
  expect((screen.getByRole('combobox',{name:'外观'}) as HTMLSelectElement).value).toBe('system');
 });
 it('keeps the open search layout for results without applying it to settings',async()=>{
  vi.stubGlobal('fetch',vi.fn(async(url:RequestInfo|URL)=>{
   const u=String(url);
   const data=u==='/api/settings'?{download_dir:'/qa',merge_folders:false,source_mode:'auto',download_concurrency:4,requests_per_second:5}:u==='/api/favorites'?{favorites:[]}:u.includes('/catalog/subjects')?{subjects:[{code:'9702',name:'Physics'}]}:u.includes('/catalog/availability')?{seasons:[{season:'Jun',status:'available',file_count:2}],latest_available_season:'Jun'}:u.includes('/api/search')?{source:'qa',groups:[{id:'qa',number:'22',files:[]}]}:{status:'ok'};
   return new Response(JSON.stringify(data),{headers:{'content-type':'application/json'}});
  }));
  render(<App/>);
  expect(screen.getByRole('main').classList.contains('search-workspace')).toBe(true);
  fireEvent.click(await screen.findByText('Physics'));
  expect(document.activeElement).toBe(screen.getByRole('spinbutton',{name:'年份'}));
  await waitFor(()=>expect((screen.getByRole('button',{name:'查找试卷'}) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole('button',{name:'查找试卷'}));
  await screen.findByText('Paper 22');
  expect(document.querySelector('.hero.compact')).toBeTruthy();
  expect(screen.getByRole('main').classList.contains('search-workspace')).toBe(true);
  fireEvent.click(screen.getByRole('button',{name:'设置'}));
  expect(screen.getByRole('main').classList.contains('search-workspace')).toBe(false);
  fireEvent.click(screen.getByRole('button',{name:'找卷'}));
  expect(screen.getByRole('main').classList.contains('search-workspace')).toBe(true);
 });
});
