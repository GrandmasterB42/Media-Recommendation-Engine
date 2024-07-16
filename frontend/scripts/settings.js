var user_changes = document.getElementsByClassName("change-form");
for (let i = 0; i < user_changes.length; i++) {
    user_changes[i].addEventListener("htmx:afterRequest", (event) => {
        if (event.detail.successful) {
            setTimeout(() => { window.location.reload(true) }, 2000); // TODO: Find a better solution or at least have an indicator
        }
    });
}