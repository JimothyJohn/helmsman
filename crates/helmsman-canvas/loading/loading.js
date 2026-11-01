// The loading screen's logic, inlined by trunk into the head of both pages
// (the box hashes inline scripts into its CSP). It runs before anything else
// so it hears every failure, and in old-style JavaScript so that even a very
// old browser reaches the message instead of a blank page.
//
// Told by: helmsman-canvas-initializer.js (download progress, init failure)
// and the app itself (helmsman-started, helmsman-start-failed, src/main.rs).
// Must never contain a script tag's text: the box scans the page for them.
(function () {
    'use strict';

    // No progress for this long and the screen says so, and stays honest:
    // if the app then starts after all, the message goes away with the rest.
    var STALL_MS = 60000;

    var WHY = {
        load: 'A file this page needs did not load. Reload the page. If it happens again, the connection or the server may be down.',
        start: 'The page loaded, but the program could not start in this browser. Reload the page. If it happens again, try a current version of your browser.',
        unsupported: 'This browser cannot run the program: it needs WebAssembly, which is missing or turned off. Use a current version of Chrome, Edge, Firefox or Safari.',
        stalled: 'This is taking longer than it should. The connection may be slow or down. You can keep waiting, or reload the page.'
    };

    var started = false;
    var failure = null; // { kind, details: [string] }
    var text = 'Loading Helmsman…';
    var percent = null; // null: the bar shows activity, not an amount
    var stall = null;

    function el(id) {
        return document.getElementById(id);
    }

    function render() {
        if (!el('loading')) {
            return; // not parsed yet: DOMContentLoaded renders
        }
        var bar = el('loading-bar');
        el('loading-text').textContent = text;
        bar.className = percent === null ? 'indeterminate' : '';
        el('loading-fill').style.width = percent === null ? '' : percent + '%';
        if (percent === null) {
            bar.removeAttribute('aria-valuenow');
        } else {
            bar.setAttribute('aria-valuenow', String(percent));
        }
        var showing = failure !== null;
        el('loading-progress').hidden = showing;
        el('loading-failed').hidden = !showing;
        if (showing) {
            el('loading-why').textContent = WHY[failure.kind];
            el('loading-detail').textContent = failure.details.join('\n');
            el('loading-detail').hidden = failure.details.length === 0;
        }
    }

    function fail(kind, detail) {
        if (started) {
            return;
        }
        if (failure === null || (failure.kind === 'stalled' && kind !== 'stalled')) {
            failure = { kind: kind, details: failure ? failure.details : [] };
        }
        if (detail && failure.details.indexOf(detail) < 0) {
            failure.details.push(detail);
        }
        render();
    }

    // The program's address, from the preload link trunk puts in the page.
    function wasmUrl() {
        var link = document.querySelector('link[rel="preload"][href$=".wasm"]');
        return link ? link.href : '';
    }

    function watch() {
        clearTimeout(stall);
        stall = setTimeout(function () {
            fail('stalled', wasmUrl());
        }, STALL_MS);
    }

    function megabytes(bytes) {
        return (bytes / 1048576).toFixed(1) + ' MB';
    }

    function messageOf(reason) {
        return String((reason && reason.message) || reason);
    }

    function kindOf(message) {
        return (/fetch|network/i).test(message) ? 'load' : 'start';
    }

    // Capture: a script's or link's load error does not bubble.
    window.addEventListener('error', function (e) {
        var target = e.target;
        if (target && target !== window) {
            if (target.tagName === 'SCRIPT') {
                fail('load', target.src);
            } else if (target.tagName === 'LINK' && (target.rel === 'preload' || target.rel === 'modulepreload')) {
                fail('load', target.href);
            }
            return;
        }
        // Another origin's error (a browser extension) is not this page's.
        if (e.filename && e.filename.indexOf(location.origin) !== 0) {
            return;
        }
        if (e.message === 'Script error.' || (/^ResizeObserver loop/).test(e.message || '')) {
            return;
        }
        fail('start', e.message);
    }, true);

    // A download cut off half way ends here: nothing else hears it.
    window.addEventListener('unhandledrejection', function (e) {
        var message = messageOf(e.reason);
        fail(kindOf(message), message);
    });

    window.addEventListener('helmsman-start-failed', function (e) {
        fail('start', messageOf(e.detail));
    });

    window.addEventListener('helmsman-started', function () {
        started = true;
        clearTimeout(stall);
        var box = el('loading');
        if (box) {
            box.parentNode.removeChild(box);
        }
    });

    document.addEventListener('DOMContentLoaded', function () {
        el('loading-reload').addEventListener('click', function () {
            location.reload();
        });
        render();
    });

    window.helmsmanLoading = {
        progress: function (current, total) {
            if (failure !== null && failure.kind === 'stalled') {
                failure = null;
            }
            watch();
            if (total > 0 && current <= total) {
                percent = Math.floor((current / total) * 100);
                text = current === total
                    ? 'Starting…'
                    : 'Downloading: ' + megabytes(current) + ' of ' + megabytes(total) + ' (' + percent + '%)';
            } else {
                percent = null;
                text = 'Downloading: ' + megabytes(current);
            }
            render();
        },
        failed: function (reason) {
            var message = messageOf(reason);
            fail(kindOf(message), wasmUrl());
            fail('start', message);
        }
    };

    if (typeof WebAssembly !== 'object' || typeof WebAssembly.instantiate !== 'function') {
        fail('unsupported', '');
    } else {
        watch();
    }
}());
