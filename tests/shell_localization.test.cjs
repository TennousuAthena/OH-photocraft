'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const loadPureEts = require('./load_pure_ets.cjs');
const platformRoot = path.resolve(__dirname, '../harmonyos/entry/src/main/ets/platform');
const resourceRoot = path.resolve(__dirname, '../harmonyos/entry/src/main/resources');
const locales = ['base', 'zh_Hans', 'zh_Hant'];
const catalogs = new Map(locales.map(locale => {
  const entries = JSON.parse(fs.readFileSync(path.join(resourceRoot, locale, 'element/string.json'), 'utf8')).string;
  assert.equal(new Set(entries.map(entry => entry.name)).size, entries.length, 'resource names must be unique');
  return [locale, new Map(entries.map(entry => [entry.name, entry.value]))];
}));
const placeholders = value => [...value.matchAll(/\{\d+\}/g)].map(match => match[0]).sort();

test('shell resources cover every language with matching placeholders and file picker extensions', () => {
  const base = catalogs.get('base');
  for (const locale of locales.slice(1)) {
    const localized = catalogs.get(locale);
    assert.deepEqual([...localized.keys()].sort(), [...base.keys()].sort());
    for (const [name, value] of base) {
      assert.deepEqual(placeholders(localized.get(name)), placeholders(value), name + ' ' + locale);
      if (value.includes('|.')) {
        assert.equal(localized.get(name).split('|')[1], value.split('|')[1], 'file type extensions must stay stable');
      }
    }
  }
  for (const file of [...fs.readdirSync(platformRoot).filter(name => name.endsWith('.ets')), '../pages/Index.ets']) {
    const source = fs.readFileSync(path.join(platformRoot, file), 'utf8');
    for (const match of source.matchAll(/shellString\(['"]([^'"]+)['"],\s*("(?:[^"\\]|\\.)*")/g)) {
      assert.equal(base.get(match[1]), JSON.parse(match[2]), file + ': ' + match[1]);
    }
    for (const match of source.matchAll(/\$r\(['"]app\.string\.([^'"]+)['"]\)/g)) {
      assert(base.has(match[1]), file + ': missing resource ' + match[1]);
    }
  }
});

test('messages follow the current resource locale each time without caching translated text', () => {
  const {configureShellLocalization, shellString} = loadPureEts(path.join(platformRoot, 'ShellLocalization.ets'));
  let locale = 'base';
  configureShellLocalization(key => catalogs.get(locale).get(key));
  const key = 'shell_the_clipboard_contains_no_image_to_paste';
  assert.equal(shellString(key, 'fallback'), 'The clipboard contains no image to paste');
  locale = 'zh_Hans';
  assert.equal(shellString(key, 'fallback'), '剪贴板中没有可粘贴的图片');
  locale = 'zh_Hant';
  assert.equal(shellString(key, 'fallback'), '剪貼簿中沒有可貼上的圖片');
  configureShellLocalization(undefined);
  assert.equal(shellString(key, 'English fallback'), 'English fallback');
});

test('missing resources preserve operational errors and formatting never rewrites inserted details', () => {
  const {configureShellLocalization, shellString} = loadPureEts(path.join(platformRoot, 'ShellLocalization.ets'));
  configureShellLocalization(() => { throw new Error('resource missing'); });
  assert.equal(shellString('missing', 'Operation failed: {0}', 'disk error'), 'Operation failed: disk error');
  assert.equal(shellString('missing', '{0}; {0}; {1}', 'literal {1}', 7), 'literal {1}; literal {1}; 7');
  assert.equal(shellString('missing', 'Missing value: {2}', 'unused'), 'Missing value: {2}');
  configureShellLocalization(key => catalogs.get('zh_Hans').get(key));
  const key = [...catalogs.get('base')].find(([, value]) => value === 'Could not prepare the output folder (error {0}). Select it again and retry')[0];
  assert.equal(shellString(key, 'fallback {0}', 13900020), '无法准备输出目录（错误 13900020），请重新选择后重试');
});
