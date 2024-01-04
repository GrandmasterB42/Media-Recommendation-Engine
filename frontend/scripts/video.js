var ws;
var isUserSeek = true;
var justJoined = true;

// NOTE: Using the HTMX Websocket is probably not the easiest thing right now, but I hope it will help with the rest of the UI
// TODO: the websocket seems to be undefined sometimes
document.body.addEventListener("htmx:wsOpen", function (event) {
    ws = event.detail.socketWrapper;
});

document.body.addEventListener("htmx:wsBeforeMessage", function (event) {
    try {
        if (ws == undefined) {
            ws = event.detail.socketWrapper;
        }
        var data = JSON.parse(event.detail.message);
        handleServerEvent(data)
    } catch (e) {
        // Errors can mostly be ignored as they wouldn't be fatal in any way
    }
});

var video = document.getElementById("currentvideo");

video.addEventListener("play", function () {
    sendVideoState("Play", this.currentTime);
})

video.addEventListener("pause", function () {
    sendVideoState("Pause", this.currentTime);
})

video.addEventListener("seeked", function () {
    if (!justJoined) {
        if (isUserSeek) {
            sendVideoState("Seek", this.currentTime)
        }
        isUserSeek = true;
    }
})

function sendVideoState(state, time) {
    if (justJoined) {
        var message = {};
        message["Join"] = null;
        ws.send(JSON.stringify(message));
        return;
    }
    var message = {};
    message[state] = time !== undefined ? time : null;
    var message = JSON.stringify(message);
    ws.send(message);
}

function handleServerEvent(data) {
    if (data === "Join") {
        if (justJoined) {
            justJoined = false;
            return;
        }
        isUserSeek = false;
        sendVideoState("Seek", video.currentTime)
    } else if (data.Play) {
        isUserSeek = false;
        video.currentTime = data.Play;
        try {
            video.play()
        } catch (e) {
            // I know this will fail before first interaction
        }
    } else if (data.Pause) {
        isUserSeek = false;
        video.currentTime = data.Pause
        video.pause()
    } else if (data.Seek) {
        isUserSeek = false;
        video.currentTime = data.Seek
    } else if (data.State) {
        if (data.State === "Playing") {
            try {
                video.play();
            } catch (e) {
                // I know this will fail before first interaction
            }
        } else if (data.State === "Paused") {
            video.pause();
        }
    }
}