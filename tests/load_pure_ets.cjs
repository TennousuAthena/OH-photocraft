'use strict';

// Transpile pure ArkTS helpers and their relative imports using the installed SDK compiler.
const fs = require('node:fs');
const path = require('node:path');
const Module = require('node:module');
const ts = require(process.env.ARKTS_TYPESCRIPT_PATH ||
  '/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/ets/build-tools/ets-loader/node_modules/typescript/lib/typescript.js');

module.exports = function loadPureEts(filename) {
  const modules = new Map();
  function load(source) {
    if (modules.has(source)) return modules.get(source).exports;
    const loaded = new Module(source, module);
    modules.set(source, loaded);
    loaded.filename = source;
    loaded.require = name => {
      if (name.startsWith('./')) return load(path.resolve(path.dirname(source), name + '.ets'));
      throw new Error('Unexpected platform import in a pure helper: ' + name);
    };
    loaded._compile(ts.transpileModule(fs.readFileSync(source, 'utf8'), {
      compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022}
    }).outputText, source);
    return loaded.exports;
  }
  return load(filename);
};
