#!/usr/bin/env node

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

global.window = {
  setTimeout,
  clearTimeout,
};
global.document = {
  createElement() {
    return {
      set textContent(value) {
        this._textContent = value;
      },
      get innerHTML() {
        return this._textContent || '';
      },
    };
  },
};

const scriptPath = path.join(__dirname, '..', 'static', 'anilist-media-autocomplete.js');
vm.runInThisContext(fs.readFileSync(scriptPath, 'utf8'), { filename: scriptPath });

const helpers = window.BeeMediaAutocomplete._test;
const fixture = {
  media: [
    {
      id: 9253,
      url: 'https://anilist.co/anime/9253',
      type: 'ANIME',
      titles: {
        romaji: 'Steins;Gate',
        english: 'Steins;Gate',
        native: 'シュタインズ・ゲート',
        userPreferred: 'Steins;Gate',
      },
      synonyms: ['StG', 'Steins Gate'],
      format: 'TV',
      year: 2011,
      popularity: 493871,
      search: 'steins gate stg',
    },
    {
      id: 16498,
      url: 'https://anilist.co/anime/16498',
      type: 'ANIME',
      titles: {
        romaji: 'Shingeki no Kyojin',
        english: 'Attack on Titan',
        native: '進撃の巨人',
        userPreferred: 'Shingeki no Kyojin',
      },
      synonyms: ['AoT', 'SNK'],
      format: 'TV',
      year: 2013,
      popularity: 632119,
      search: 'shingeki no kyojin attack on titan 進撃の巨人 aot snk',
    },
  ],
};

assert.strictEqual(helpers.normalizeQuery(' Steins;Gate!! '), 'steins gate');

let results = helpers.searchStaticItems(fixture, 'steins', 8);
assert.strictEqual(results[0].id, 9253);
assert.strictEqual(results[0].source, 'static');

results = helpers.searchStaticItems(fixture, 'Attack Titan', 8);
assert.strictEqual(results[0].id, 16498);

results = helpers.searchStaticItems(fixture, '進撃', 8);
assert.strictEqual(results[0].id, 16498);

results = helpers.searchStaticItems(fixture, 'AoT', 8);
assert.strictEqual(results[0].id, 16498);

assert.strictEqual(helpers.shouldUseLiveFallback([]), true);
assert.strictEqual(helpers.shouldUseLiveFallback([{ matchScore: 40 }]), true);
assert.strictEqual(helpers.shouldUseLiveFallback([{ matchScore: 80 }]), false);

const merged = helpers.mergeResults(
  [{ id: 9253, type: 'ANIME' }],
  [{ id: 9253, type: 'ANIME' }, { id: 30002, type: 'MANGA' }],
  8,
);
assert.deepStrictEqual(merged.map(item => item.id), [9253, 30002]);

console.log('media autocomplete helper tests passed');
