(function (root) {
    'use strict';

    const STATIC_INDEX_URL = '/static/data/anilist-media-index.json';
    const LIVE_SEARCH_URL = '/api/anilist-media-search';
    const VNDB_SEARCH_URL = '/api/vndb-media-search';
    const MIN_QUERY_CHARS = 2;
    const MAX_RESULTS = 8;
    const DEBOUNCE_MS = 220;
    const USEFUL_STATIC_SCORE = 65;

    let staticIndexPromise = null;
    const liveCache = new Map();

    function normalizeQuery(value) {
        return String(value || '')
            .normalize('NFKC')
            .toLowerCase()
            .replace(/[^\p{L}\p{N}]+/gu, ' ')
            .trim();
    }

    function titleValues(item) {
        const titles = item && item.titles ? item.titles : {};
        return [
            titles.userPreferred,
            titles.native,
            titles.romaji,
            titles.english,
        ].filter(Boolean);
    }

    function uniqueValues(values) {
        const seen = new Set();
        const result = [];
        values.forEach(value => {
            const key = normalizeQuery(value);
            if (!key || seen.has(key)) return;
            seen.add(key);
            result.push(value);
        });
        return result;
    }

    function displayTitle(item) {
        return titleValues(item)[0] || `AniList #${item.id}`;
    }

    function secondaryTitle(item) {
        const primary = normalizeQuery(displayTitle(item));
        return titleValues(item).find(title => normalizeQuery(title) !== primary) || '';
    }

    function itemType(item) {
        const type = String(item.type || item.media_type || 'ANIME').toUpperCase();
        if (type === 'VN' || type === 'VNDB') return 'VN';
        return type === 'MANGA' ? 'MANGA' : 'ANIME';
    }

    function itemUrl(item) {
        if (item.url) return item.url;
        if (itemType(item) === 'VN') return `https://vndb.org/${item.id}`;
        const domain = itemType(item) === 'MANGA' ? 'manga' : 'anime';
        return `https://anilist.co/${domain}/${item.id}`;
    }

    function normalizedResult(item, source, matchScore) {
        return {
            id: item.id,
            url: itemUrl(item),
            type: itemType(item),
            titles: item.titles || {},
            synonyms: Array.isArray(item.synonyms) ? item.synonyms : [],
            format: item.format || null,
            year: item.year || null,
            popularity: Number(item.popularity) || 0,
            source,
            matchScore: matchScore || 0,
        };
    }

    function nonSpaceLength(query) {
        return String(query || '').replace(/\s+/g, '').length;
    }

    function looksLikeDirectAnilistId(query) {
        const value = String(query || '').trim();
        return /^\d+$/.test(value) || /anilist\.co\/(anime|manga)\/\d+/i.test(value);
    }

    function looksLikeDirectVndbId(query) {
        const value = String(query || '').trim();
        return /^v?\d+$/i.test(value) || /vndb\.org\/v\d+/i.test(value);
    }

    function scoreStaticItem(item, normalizedQuery) {
        if (!normalizedQuery) return 0;

        const fields = uniqueValues([
            ...titleValues(item),
            ...(Array.isArray(item.synonyms) ? item.synonyms : []),
        ]).map(normalizeQuery);
        const search = item.search ? String(item.search) : fields.join(' ');
        const queryTokens = normalizedQuery.split(/\s+/).filter(Boolean);

        let score = 0;
        fields.forEach(field => {
            if (field === normalizedQuery) score = Math.max(score, 120);
            if (field.startsWith(normalizedQuery)) score = Math.max(score, 100);
            if (field.split(/\s+/).some(token => token.startsWith(normalizedQuery))) {
                score = Math.max(score, 80);
            }
            if (field.includes(normalizedQuery)) score = Math.max(score, 70);
        });

        if (search.includes(normalizedQuery)) score = Math.max(score, 65);
        if (queryTokens.length > 1 && queryTokens.every(token => search.includes(token))) {
            score = Math.max(score, 55);
        }

        return score;
    }

    function searchStaticItems(indexOrItems, query, limit = MAX_RESULTS) {
        const normalized = normalizeQuery(query);
        if (nonSpaceLength(normalized) < MIN_QUERY_CHARS) return [];

        const items = Array.isArray(indexOrItems)
            ? indexOrItems
            : ((indexOrItems && indexOrItems.media) || []);

        return items
            .map(item => ({ item, score: scoreStaticItem(item, normalized) }))
            .filter(match => match.score > 0 && itemType(match.item) === 'ANIME')
            .sort((a, b) => {
                if (b.score !== a.score) return b.score - a.score;
                const popularityDelta = (Number(b.item.popularity) || 0) - (Number(a.item.popularity) || 0);
                if (popularityDelta !== 0) return popularityDelta;
                return displayTitle(a.item).localeCompare(displayTitle(b.item));
            })
            .slice(0, limit)
            .map(match => normalizedResult(match.item, 'static', match.score));
    }

    function shouldUseLiveFallback(staticResults) {
        return staticResults.length === 0 || (staticResults[0].matchScore || 0) < USEFUL_STATIC_SCORE;
    }

    function mergeResults(staticResults, liveResults, limit = MAX_RESULTS) {
        const seen = new Set();
        const merged = [];
        [...staticResults, ...liveResults].forEach(item => {
            const key = `${item.type}:${item.id}`;
            if (seen.has(key)) return;
            seen.add(key);
            merged.push(item);
        });
        return merged.slice(0, limit);
    }

    async function loadStaticIndex() {
        if (!staticIndexPromise) {
            staticIndexPromise = fetch(STATIC_INDEX_URL)
                .then(response => response.ok ? response.json() : { media: [] })
                .catch(() => ({ media: [] }));
        }
        return staticIndexPromise;
    }

    async function searchLive(query, mediaType, signal) {
        const normalized = normalizeQuery(query);
        if (nonSpaceLength(normalized) < MIN_QUERY_CHARS) return [];

        const key = `${mediaType}:${normalized}`;
        if (liveCache.has(key)) return liveCache.get(key);

        const params = new URLSearchParams({ q: query.trim(), media_type: mediaType });
        const response = await fetch(`${LIVE_SEARCH_URL}?${params.toString()}`, { signal });
        if (!response.ok) return [];

        const payload = await response.json();
        const results = (payload.results || [])
            .slice(0, MAX_RESULTS)
            .map(item => normalizedResult(item, 'live', 0));
        liveCache.set(key, results);
        return results;
    }

    async function searchVndb(query, signal) {
        const normalized = normalizeQuery(query);
        if (nonSpaceLength(normalized) < MIN_QUERY_CHARS) return [];

        const key = `VN:${normalized}`;
        if (liveCache.has(key)) return liveCache.get(key);

        const params = new URLSearchParams({ q: query.trim() });
        const response = await fetch(`${VNDB_SEARCH_URL}?${params.toString()}`, { signal });
        if (!response.ok) return [];

        const payload = await response.json();
        const results = (payload.results || [])
            .slice(0, MAX_RESULTS)
            .map(item => normalizedResult(item, 'vndb', 0));
        liveCache.set(key, results);
        return results;
    }

    function clearList(state) {
        state.results = [];
        state.activeIndex = -1;
        state.list.innerHTML = '';
        state.wrapper.hidden = true;
        state.input.setAttribute('aria-expanded', 'false');
    }

    function renderList(state, results) {
        state.results = results;
        state.activeIndex = results.length ? 0 : -1;
        state.list.innerHTML = '';

        if (results.length === 0) {
            clearList(state);
            return;
        }

        results.forEach((item, index) => {
            const button = document.createElement('button');
            button.type = 'button';
            button.className = 'media-autocomplete-option';
            button.setAttribute('role', 'option');
            button.setAttribute('aria-selected', index === state.activeIndex ? 'true' : 'false');
            button.dataset.index = String(index);

            const parts = [item.format, item.year].filter(Boolean).join(' / ');
            const secondary = secondaryTitle(item);
            const sourceLabel = item.source === 'static'
                ? 'Index'
                : item.source === 'vndb'
                    ? 'VNDB'
                    : 'Live';
            button.innerHTML = `
                <span class="media-autocomplete-main">
                    <span class="media-autocomplete-title">${escapeHtml(displayTitle(item))}</span>
                    ${secondary ? `<span class="media-autocomplete-secondary">${escapeHtml(secondary)}</span>` : ''}
                </span>
                <span class="media-autocomplete-meta">
                    ${parts ? `<span>${escapeHtml(parts)}</span>` : ''}
                    <span class="media-autocomplete-badge ${item.source}">${sourceLabel}</span>
                </span>
            `;
            button.addEventListener('mousedown', event => {
                event.preventDefault();
                selectResult(state, item);
            });
            state.list.appendChild(button);
        });

        state.wrapper.hidden = false;
        state.input.setAttribute('aria-expanded', 'true');
        updateActiveOption(state);
    }

    function updateActiveOption(state) {
        const options = state.list.querySelectorAll('.media-autocomplete-option');
        options.forEach((option, index) => {
            const active = index === state.activeIndex;
            option.classList.toggle('active', active);
            option.setAttribute('aria-selected', active ? 'true' : 'false');
        });
    }

    function selectResult(state, item) {
        const sourceSelect = state.row.querySelector('[data-field="source"]');
        const mediaTypeSelect = state.row.querySelector('[data-field="media_type"]');

        const resultType = itemType(item);
        const targetSource = resultType === 'VN' ? 'vndb' : 'anilist';

        if (sourceSelect && sourceSelect.value !== targetSource) {
            sourceSelect.value = targetSource;
            if (typeof window.onEntrySourceChange === 'function') {
                window.onEntrySourceChange(sourceSelect);
            }
        }

        if (mediaTypeSelect && resultType !== 'VN') {
            mediaTypeSelect.value = resultType === 'MANGA' ? 'MANGA' : 'ANIME';
        }

        state.input.value = item.url;
        state.input.dataset.mediaTitle = displayTitle(item);
        state.input.dataset.mediaTitleRomaji = item.titles.romaji || displayTitle(item);
        clearList(state);

        if (typeof state.options.validate === 'function') {
            state.options.validate(state.input);
        }
        if (typeof state.options.onChange === 'function') {
            state.options.onChange(state.row, item);
        }
    }

    function scheduleSearch(state) {
        window.clearTimeout(state.timer);
        state.timer = window.setTimeout(() => runSearch(state), DEBOUNCE_MS);
    }

    async function runSearch(state) {
        const sourceSelect = state.row.querySelector('[data-field="source"]');
        const mediaTypeSelect = state.row.querySelector('[data-field="media_type"]');
        const source = sourceSelect ? sourceSelect.value : '';
        const mediaType = mediaTypeSelect && mediaTypeSelect.value === 'MANGA' ? 'MANGA' : 'ANIME';
        const query = state.input.value.trim();

        if (
            !sourceSelect ||
            nonSpaceLength(query) < MIN_QUERY_CHARS ||
            (source === 'anilist' && looksLikeDirectAnilistId(query)) ||
            (source === 'vndb' && looksLikeDirectVndbId(query))
        ) {
            clearList(state);
            return;
        }

        state.sequence += 1;
        const sequence = state.sequence;
        if (state.abortController) {
            state.abortController.abort();
        }
        state.abortController = new AbortController();

        let results = [];
        if (source === 'vndb') {
            try {
                results = await searchVndb(query, state.abortController.signal);
            } catch (error) {
                if (error.name === 'AbortError') return;
                results = [];
            }
        } else if (source === 'anilist') {
            let staticResults = [];
            if (mediaType === 'ANIME') {
                staticResults = searchStaticItems(await loadStaticIndex(), query, MAX_RESULTS);
            }

            results = staticResults;
            if (mediaType === 'MANGA' || shouldUseLiveFallback(staticResults)) {
                try {
                    const liveResults = await searchLive(query, mediaType, state.abortController.signal);
                    results = mergeResults(staticResults, liveResults, MAX_RESULTS);
                } catch (error) {
                    if (error.name === 'AbortError') return;
                    results = staticResults;
                }
            }
        } else {
            clearList(state);
            return;
        }

        if (sequence !== state.sequence) return;
        renderList(state, results);
    }

    function handleKeydown(state, event) {
        if (state.wrapper.hidden || state.results.length === 0) return;

        if (event.key === 'ArrowDown') {
            event.preventDefault();
            state.activeIndex = (state.activeIndex + 1) % state.results.length;
            updateActiveOption(state);
        } else if (event.key === 'ArrowUp') {
            event.preventDefault();
            state.activeIndex = (state.activeIndex - 1 + state.results.length) % state.results.length;
            updateActiveOption(state);
        } else if (event.key === 'Enter') {
            if (state.activeIndex >= 0) {
                event.preventDefault();
                selectResult(state, state.results[state.activeIndex]);
            }
        } else if (event.key === 'Escape') {
            clearList(state);
        }
    }

    function attach(row, options = {}) {
        if (!row || row.dataset.mediaAutocompleteAttached === 'true') return;

        const entryId = row.querySelector('.entry-id');
        const input = row.querySelector('[data-field="id"]');
        if (!entryId || !input) return;

        row.dataset.mediaAutocompleteAttached = 'true';
        input.setAttribute('autocomplete', 'off');
        input.setAttribute('aria-autocomplete', 'list');
        input.setAttribute('aria-expanded', 'false');

        const wrapper = document.createElement('div');
        wrapper.className = 'media-autocomplete';
        wrapper.hidden = true;
        wrapper.innerHTML = '<div class="media-autocomplete-list" role="listbox"></div>';
        entryId.insertBefore(wrapper, input.nextSibling);

        const state = {
            row,
            input,
            wrapper,
            list: wrapper.querySelector('.media-autocomplete-list'),
            options,
            timer: null,
            results: [],
            activeIndex: -1,
            abortController: null,
            sequence: 0,
        };

        row._mediaAutocomplete = state;
        input.addEventListener('input', () => {
            delete input.dataset.mediaTitle;
            delete input.dataset.mediaTitleRomaji;
            scheduleSearch(state);
        });
        input.addEventListener('focus', () => scheduleSearch(state));
        input.addEventListener('keydown', event => handleKeydown(state, event));

        const sourceSelect = row.querySelector('[data-field="source"]');
        if (sourceSelect) {
            sourceSelect.addEventListener('change', () => scheduleSearch(state));
        }
        const mediaTypeSelect = row.querySelector('[data-field="media_type"]');
        if (mediaTypeSelect) {
            mediaTypeSelect.addEventListener('change', () => scheduleSearch(state));
        }

        document.addEventListener('mousedown', event => {
            if (!row.contains(event.target)) clearList(state);
        });
    }

    function refresh(row) {
        const state = row && row._mediaAutocomplete;
        if (state) scheduleSearch(state);
    }

    function escapeHtml(value) {
        const div = document.createElement('div');
        div.textContent = value == null ? '' : String(value);
        return div.innerHTML;
    }

    root.BeeMediaAutocomplete = {
        attach,
        refresh,
        _test: {
            normalizeQuery,
            scoreStaticItem,
            searchStaticItems,
            shouldUseLiveFallback,
            mergeResults,
            itemType,
            itemUrl,
            displayTitle,
            secondaryTitle,
            looksLikeDirectAnilistId,
            looksLikeDirectVndbId,
        },
    };

    if (typeof module !== 'undefined' && module.exports) {
        module.exports = root.BeeMediaAutocomplete;
    }
})(globalThis);
