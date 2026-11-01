// trunk's data-initializer: it fetches the program and calls these. Its
// `total` is the size of the built file, so the bar is exact even when the
// server compresses the download and sends no usable Content-Length.
// loading.js, in the page's head, owns the screen.
export default function () {
    return {
        onProgress: ({ current, total }) => window.helmsmanLoading.progress(current, total),
        onFailure: (reason) => window.helmsmanLoading.failed(reason),
    };
}
