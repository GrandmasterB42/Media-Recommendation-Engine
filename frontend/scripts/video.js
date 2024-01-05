var ws;
var isUserEvent = true;
var justJoined = true;

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
    if (isUserEvent) {
        sendVideoState("Play", this.currentTime);
    }
    isUserEvent = false;
})

video.addEventListener("pause", function () {
    if (isUserEvent) {
        sendVideoState("Pause", this.currentTime);
    }
    isUserEvent = false
})

video.addEventListener("seeked", function () {
    if (!justJoined) {
        if (isUserEvent) {
            sendVideoState("Seek", this.currentTime)
        }
        isUserEvent = true;
    }
})

function sendVideoState(state, time) {
    if (justJoined) {
        var message = {};
        message["Join"] = true;
        ws.send(JSON.stringify(message));
        return;
    }
    var message = {};
    message[state] = time !== undefined ? time : null;
    var message = JSON.stringify(message);
    ws.send(message);
}

function handleServerEvent(data) {
    if (data.Join) {
        if (justJoined) {
            justJoined = false;
            return;
        }
        isUserEvent = false;
        sendVideoState("Seek", video.currentTime)
    } else if (data.Play) {
        isUserEvent = false;
        video.currentTime = data.Play;
        try {
            video.play()
        } catch (e) {
            // I know this will fail before first interaction
        }
    } else if (data.Pause) {
        isUserEvent = false;
        video.currentTime = data.Pause
        video.pause()
    } else if (data.Seek) {
        isUserEvent = false;
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