document.body.addEventListener("htmx:beforeSwap", (event) => {
    if (event.detail.xhr.status === 500) {
        event.detail.shouldSwap = true;
        event.detail.isError = false;
    }
});