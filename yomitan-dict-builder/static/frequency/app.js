let lastIndexUrl = '';

function buildFrequencyParams() {
    const vndbUser = document.getElementById('vndbUser').value.trim();
    const anilistUser = document.getElementById('anilistUser').value.trim();
    const params = new URLSearchParams();

    if (vndbUser) params.set('vndb_user', vndbUser);
    if (anilistUser) params.set('anilist_user', anilistUser);

    return params;
}

async function fetchFrequencyLists() {
    const params = buildFrequencyParams();
    const status = document.getElementById('status');
    const preview = document.getElementById('mediaPreview');
    const fetchBtn = document.getElementById('fetchListsBtn');

    clearUnmatched();
    hideResult();

    if (!params.toString()) {
        setStatus(status, 'Please enter at least one VNDB or AniList username.', 'error');
        return;
    }

    fetchBtn.disabled = true;
    fetchBtn.textContent = 'Fetching...';
    preview.classList.remove('show');
    setStatus(status, 'Fetching current VNDB/AniList media...', 'loading');

    try {
        const response = await fetch('/api/user-lists?' + params.toString());
        const data = await response.json();
        if (!response.ok || data.error) {
            throw new Error(data.error || `HTTP ${response.status}`);
        }

        renderMediaPreview(data.entries || []);

        let message = `Found ${(data.entries || []).length} current title${(data.entries || []).length === 1 ? '' : 's'}.`;
        if (data.errors && data.errors.length > 0) {
            message += ` Warnings: ${data.errors.join('; ')}`;
        }
        setStatus(status, message, 'success');
        updateIndexUrl();
    } catch (err) {
        setStatus(status, `Error: ${err.message}`, 'error');
    } finally {
        fetchBtn.disabled = false;
        fetchBtn.textContent = 'Fetch Lists & Preview';
    }
}

function generateFrequencyDictionary() {
    const params = buildFrequencyParams();
    const status = document.getElementById('status');
    const generateBtn = document.getElementById('generateBtn');
    const fetchBtn = document.getElementById('fetchListsBtn');
    const progressContainer = document.getElementById('progressContainer');
    const progressBar = document.getElementById('progressBar');

    clearUnmatched();
    hideResult();

    if (!params.toString()) {
        setStatus(status, 'Please enter at least one VNDB or AniList username.', 'error');
        return;
    }

    updateIndexUrl();
    generateBtn.disabled = true;
    fetchBtn.disabled = true;
    generateBtn.textContent = 'Generating...';
    progressContainer.classList.add('show');
    progressBar.style.width = '0%';
    progressBar.textContent = '';
    setStatus(status, 'Starting frequency dictionary generation...', 'loading');

    const eventSource = new EventSource('/api/generate-frequency-stream?' + params.toString());

    eventSource.addEventListener('progress', (event) => {
        const data = JSON.parse(event.data);
        const total = Math.max(Number(data.total) || 1, 1);
        const current = Math.min(Number(data.current) || 0, total);
        const pct = Math.round((current / total) * 100);
        progressBar.style.width = pct + '%';
        progressBar.textContent = `${current}/${total}`;
        setStatus(status, `${data.stage}: ${data.title}`, 'loading');
    });

    eventSource.addEventListener('warning', (event) => {
        const data = JSON.parse(event.data);
        if (data.unmatched) {
            renderUnmatched(data.unmatched);
        }
    });

    eventSource.addEventListener('complete', async (event) => {
        eventSource.close();
        const data = JSON.parse(event.data);
        progressBar.style.width = '100%';
        progressBar.textContent = 'Done!';

        if (data.unmatched) {
            renderUnmatched(data.unmatched);
        }

        try {
            const response = await fetch('/api/download?token=' + encodeURIComponent(data.token));
            if (!response.ok) throw new Error('Download failed');

            const blob = await response.blob();
            const downloadUrl = window.URL.createObjectURL(blob);
            const a = document.createElement('a');
            a.href = downloadUrl;
            a.download = data.filename || 'bee_frequency.zip';
            document.body.appendChild(a);
            a.click();
            a.remove();
            window.URL.revokeObjectURL(downloadUrl);

            showResult();
            const matched = Number(data.matchedCount) || 0;
            const terms = Number(data.termCount) || 0;
            setStatus(status, `Frequency dictionary downloaded. Matched ${matched} title${matched === 1 ? '' : 's'} and combined ${terms} term${terms === 1 ? '' : 's'}.`, 'success');
        } catch (err) {
            setStatus(status, `Download error: ${err.message}`, 'error');
        } finally {
            generateBtn.disabled = false;
            fetchBtn.disabled = false;
            generateBtn.textContent = 'Generate Frequency Dictionary';
        }
    });

    eventSource.addEventListener('error', (event) => {
        if (event.data) {
            const data = JSON.parse(event.data);
            setStatus(status, `Error: ${data.error}`, 'error');
        } else {
            setStatus(status, 'Connection error. Please try again.', 'error');
        }
        eventSource.close();
        generateBtn.disabled = false;
        fetchBtn.disabled = false;
        generateBtn.textContent = 'Generate Frequency Dictionary';
        progressContainer.classList.remove('show');
    });

    eventSource.onerror = () => {
        if (generateBtn.disabled) {
            eventSource.close();
            setStatus(status, 'Connection lost. Please try again.', 'error');
            generateBtn.disabled = false;
            fetchBtn.disabled = false;
            generateBtn.textContent = 'Generate Frequency Dictionary';
            progressContainer.classList.remove('show');
        }
    };
}

function renderMediaPreview(entries) {
    const preview = document.getElementById('mediaPreview');
    const header = document.getElementById('mediaPreviewHeader');
    const list = document.getElementById('mediaPreviewList');

    list.innerHTML = '';
    header.textContent = `Consumed Media (${entries.length})`;

    if (entries.length === 0) {
        preview.classList.remove('show');
        return;
    }

    entries.forEach(entry => {
        const item = document.createElement('div');
        item.className = 'media-item';
        item.innerHTML = mediaMarkup(entry);
        list.appendChild(item);
    });

    preview.classList.add('show');
}

function renderUnmatched(unmatched) {
    const panel = document.getElementById('unmatchedPanel');
    const list = document.getElementById('unmatchedList');

    list.innerHTML = '';
    if (!unmatched || unmatched.length === 0) {
        panel.classList.remove('show');
        return;
    }

    unmatched.forEach(entry => {
        const item = document.createElement('div');
        item.className = 'unmatched-item';
        item.innerHTML = `
            ${mediaMarkup(entry)}
            <div class="unmatched-reason">${escapeHtml(entry.reason || 'No matching Jiten frequency deck found')}</div>
        `;
        list.appendChild(item);
    });

    panel.classList.add('show');
}

function mediaMarkup(entry) {
    const badgeClass = entry.source === 'vndb' ? 'vndb' : entry.media_type;
    const badgeText = entry.source === 'vndb' ? 'VN' :
        entry.media_type === 'anime' ? 'Anime' : 'Manga';
    const title = entry.title || entry.id || 'Untitled';
    const romaji = entry.title_romaji && entry.title_romaji !== title
        ? `<div class="media-romaji">${escapeHtml(entry.title_romaji)}</div>`
        : '';

    return `
        <div class="media-main">
            <div class="media-title">${escapeHtml(title)}</div>
            ${romaji}
        </div>
        <span class="badge ${escapeHtml(badgeClass || '')}">${escapeHtml(badgeText)}</span>
    `;
}

function updateIndexUrl() {
    const params = buildFrequencyParams();
    lastIndexUrl = params.toString()
        ? `${location.origin}/api/yomitan-frequency-index?${params.toString()}`
        : '';
    const input = document.getElementById('indexUrl');
    input.value = lastIndexUrl;
    return lastIndexUrl;
}

function showResult() {
    updateIndexUrl();
    if (lastIndexUrl) {
        document.getElementById('resultPanel').classList.add('show');
    }
}

function hideResult() {
    document.getElementById('resultPanel').classList.remove('show');
}

function clearUnmatched() {
    document.getElementById('unmatchedList').innerHTML = '';
    document.getElementById('unmatchedPanel').classList.remove('show');
}

async function copyIndexUrl() {
    const status = document.getElementById('status');
    const url = updateIndexUrl();
    if (!url) {
        setStatus(status, 'Generate a frequency URL first.', 'error');
        return;
    }

    try {
        await navigator.clipboard.writeText(url);
        setStatus(status, 'Index URL copied.', 'success');
    } catch (err) {
        const input = document.getElementById('indexUrl');
        input.focus();
        input.select();
        setStatus(status, 'Index URL selected.', 'success');
    }
}

function setStatus(el, message, type) {
    el.textContent = message;
    el.className = `status show ${type}`;
}

function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text == null ? '' : String(text);
    return div.innerHTML;
}

document.addEventListener('DOMContentLoaded', () => {
    updateIndexUrl();
});
