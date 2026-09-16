#!/usr/bin/env node

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const JSZip = require('../static/custom/jszip.min.js');

const app = vm.createContext({ JSZip, document: { addEventListener() {} } });
vm.runInContext(fs.readFileSync(path.join(__dirname, '../static/custom/app.js'), 'utf8'), app);

(async () => {
  const entries = app.parseEntries('検索, けんさく, See https://google.com for details');
  const blob = await app.buildZip('Links', entries);
  const zip = await JSZip.loadAsync(Buffer.from(await blob.arrayBuffer()));
  const bank = JSON.parse(await zip.file('term_bank_1.json').async('string'));
  assert.deepEqual(bank[0][5], [{
    type: 'structured-content',
    content: ['See ', { tag: 'a', href: 'https://google.com', content: 'https://google.com' }, ' for details'],
  }]);
  const imported = await app.parseZip(Buffer.from(await blob.arrayBuffer()));
  assert.equal(imported.rawText, '検索, けんさく, See https://google.com for details');
  for (const definition of ['', 'plain text', 'first\\n\\nlast', '<b>text</b> javascript:alert(1)',
    'https://google.com', 'HTTPS://google.com\\nhttp://example.com/?a=1&b=2#top']) {
    const blob = await app.buildZip('Links', app.parseEntries(`語, ご, ${definition}`));
    const bytes = Buffer.from(await blob.arrayBuffer());
    const zip = await JSZip.loadAsync(bytes);
    const bank = JSON.parse(await zip.file('term_bank_1.json').async('string'));
    const expectedLinks = definition.replace(/\\n/g, '\n').match(/https?:\/\/\S+/gi) || [];
    const content = bank[0][5][0]?.content || [];
    assert.deepEqual(content.filter(p => p.tag === 'a'),
      expectedLinks.map(url => ({ tag: 'a', href: url.replace(/^https?/i, s => s.toLowerCase()), content: url })));
    if (!definition.includes('\\n') && !expectedLinks.length) assert.deepEqual(bank[0][5], [definition]);
    assert.equal((await app.parseZip(bytes)).rawText, `語, ご, ${definition}`);
  }
  console.log('custom dictionary tests passed');
})().catch(error => {
  console.error(error);
  process.exitCode = 1;
});
