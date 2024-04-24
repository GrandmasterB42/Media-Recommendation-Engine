// Workaround for https://github.com/bigskysoftware/htmx/issues/2183 / https://github.com/bigskysoftware/htmx/issues/764 to fix picture-in-picture
let temp_video = document.getElementById("currentvideo");
temp_video.replaceWith(temp_video.cloneNode(true));
// ---

const video = document.getElementById("currentvideo");
const videocontainer = document.querySelector(".video-container");

document.body.addEventListener("htmx:wsBeforeMessage", (event) => {
    try {
        let data = JSON.parse(event.detail.message);
        event.preventDefault();
        handleServerEvent(data);
    } catch (e) {
        // Html gets passed on to htmx
    }
});

document.body.addEventListener("htmx:wsError", (event) => {
    console.log("Error: ", event.detail.message);
    console.log("Reloading page");
    location.reload();
});

let active = false;
async function wait_for_interact() {
    while (!navigator.userActivation.isActive) {
        await new Promise(r => setTimeout(r, 100));
    }
    active = true;
    let message = {
        "type": "Join"
    }
    ws.send(JSON.stringify(message));
}
setTimeout(wait_for_interact, 100); // Give the websocket a chance to connect

function sendVideoUpdate(type) {
    let message = {
        "type": "Update",
        "message_type": type,
        "timestamp": Date.now(),
        "video_time": video.currentTime,
        "state": video.paused ? "Paused" : "Playing"
    };
    ws.send(JSON.stringify(message));
}


let justJoined = true;
function handleServerEvent(data) {
    let type = data["type"];
    if (type === "Join") {
        if (justJoined) {
            justJoined = false;
            return;
        }
        sendVideoUpdate("Update");
    } else if (type == "Update") {
        let update_type = data["message_type"];
        let timestamp = data["timestamp"];
        let time = data["video_time"];
        let state = data["state"];

        let elapsed_since_send = Date.now() - timestamp;

        if (update_type == "Play" && active) {
            adjustvideo(state, time, elapsed_since_send);
        } else if (update_type == "Pause") {
            adjustvideo(state, time, elapsed_since_send);
        } else if (update_type == "Seek") {
            video.currentTime = time + elapsed_since_send / 1000;
        } else if (update_type == "State" && active) { // No information but the state is accurate here
            if (state === "Playing") {
                video.play();
                videocontainer.classList.remove("paused");
            } else if (state === "Paused") {
                video.pause();
                videocontainer.classList.add("paused");
            }
        } else if (update_type == "Update" && active) {
            adjustvideo(state, time, elapsed_since_send);
        }
    } else if (type == "Reload") {
        reload();
    } else {
        console.log("Unknown type: ", type);
    }
}

// TODO: The video playback still gets out of sync, even on local devices, maybe there is a flaw here that i overlookeed
function adjustvideo(new_state, new_time, elapsed_since_send) {
    if (new_state === "Playing" && !video.paused) {
        video.currentTime = new_time + elapsed_since_send / 1000;
    } else if (new_state == "Playing" && video.paused) {
        video.currentTime = new_time + elapsed_since_send / 1000;
        video.play();
        videocontainer.classList.remove("paused");
    } else if (new_state === "Paused" && !video.paused) {
        video.currentTime = new_time;
        video.pause();
        videocontainer.classList.add("paused");
    } else if (new_state === "Paused" && video.paused) {
        video.currentTime = new_time;
    }
}

// Hovering
let timeoutId;

videocontainer.addEventListener("mousemove", () => {
    if (video.paused) {
        videocontainer.classList.remove("hiddencursor");
        videocontainer.classList.remove("hovering");
        clearTimeout(timeoutId);
        timeoutId = undefined;
        return;
    }

    if (videocontainer.classList.contains("hovering")) {
        videocontainer.classList.remove("hiddencursor");
        clearTimeout(timeoutId);
    } else {
        videocontainer.classList.add("hovering");
    }

    timeoutId = setTimeout(() => {
        videocontainer.classList.remove("hovering");
        videocontainer.classList.add("hiddencursor");
    }, 4000);
});

videocontainer.addEventListener("mouseleave", () => {
    videocontainer.classList.remove("hovering");
    clearTimeout(timeoutId);
});

// Timeline
const timelinecontainer = document.querySelector(".timeline-container");
const currenttime = document.querySelector(".current-time");
const totaltime = document.querySelector(".total-time");

document.addEventListener("mouseup", e => {
    if (isScrubbing) {
        toggleScrubbing(e);
    }
});
document.addEventListener("mousemove", e => {
    if (isScrubbing) {
        handleTimelineUpdate(e);
    }
})

let wasPaused;
let isScrubbing = false;
function toggleScrubbing(e) {
    const rect = timelinecontainer.getBoundingClientRect();
    const percent = Math.min(Math.max(0, e.x - rect.x), rect.width) / rect.width;
    isScrubbing = (e.buttons & 1) === 1;
    videocontainer.classList.toggle("scrubbing", isScrubbing);
    if (isScrubbing) {
        wasPaused = video.paused;
        video.pause();
    } else {
        video.currentTime = video.duration * percent;
        if (!wasPaused) {
            video.play();
        }
        sendVideoUpdate("Seek");
    }

    handleTimelineUpdate(e);
}

function handleTimelineUpdate(e) {
    const rect = timelinecontainer.getBoundingClientRect();
    const percent = Math.min(Math.max(0, e.x - rect.x), rect.width) / rect.width;
    timelinecontainer.style.setProperty("--preview-position", percent);

    if (isScrubbing) {
        e.preventDefault();
        timelinecontainer.style.setProperty("--progress-position", percent);
    }
}

// Playback speed
const playbackspeed = document.querySelector(".speed");

function changePlaybackSpeed() {
    let newPlayRate = video.playbackRate + 0.25;
    if (newPlayRate > 2) {
        newPlayRate = 0.25;
    }
    video.playbackRate = newPlayRate;
    playbackspeed.textContent = `${newPlayRate}x`;
}


// Duration
video.addEventListener("loadedmetadata", () => {
    totaltime.innerText = formatDuration(video.duration);
})

video.addEventListener("timeupdate", () => {
    currenttime.textContent = formatDuration(video.currentTime);
    const percent = video.currentTime / video.duration;
    timelinecontainer.style.setProperty("--progress-position", percent);
})

const leadingZeroFormatter = new Intl.NumberFormat(undefined, {
    minimumIntegerDigits: 2
});

function formatDuration(duration) {
    const seconds = Math.floor(duration % 60);
    const minutes = Math.floor((duration / 60) % 60);
    const hours = Math.floor(duration / 3600);
    if (hours === 0) {
        return `${minutes}:${leadingZeroFormatter.format(seconds)}`;
    }
    return `${hours}:${leadingZeroFormatter.format(minutes)}:${leadingZeroFormatter.format(seconds)}`;
}

function skip(seconds) {
    video.currentTime += seconds;
    sendVideoUpdate("Seek");
}

// Volume
const volumeslider = document.querySelector(".volume-slider");
volumeslider.addEventListener("input", e => {
    video.volume = e.target.value;
    video.muted = e.target.value === 0;
});

function toggleMute() {
    video.muted = !video.muted;
}


video.addEventListener("volumechange", () => {
    volumeslider.value = video.volume;
    let volumeLevel;
    if (video.muted || video.volume === 0) {
        volumeslider.value = 0;
        volumeLevel = "muted";
    } else if (video.volume < 0.5) {
        volumeLevel = "low";
    } else {
        volumeLevel = "high";
    }

    videocontainer.dataset.volumeLevel = volumeLevel;
});

// Modes
document.addEventListener("fullscreenchange", () => { videocontainer.classList.toggle("full-screen", document.fullscreenElement) })
video.addEventListener("enterpictureinpicture", () => { videocontainer.classList.toggle("pip") })
video.addEventListener("leavepictureinpicture", () => { videocontainer.classList.remove("pip") })

function toggleFullscreenMode() {
    if (document.fullscreenElement == null) {
        videocontainer.requestFullscreen();
    } else {
        document.exitFullscreen();
    }
}

function togglePiPMode() {
    if (videocontainer.classList.contains("pip")) {
        document.exitPictureInPicture();
    } else {
        video.requestPictureInPicture();
    }
}

// Play / Pause
function togglePlay() {
    if (video.paused) {
        videocontainer.classList.remove("paused");
        video.play();
    } else {
        videocontainer.classList.add("paused");
        video.pause();
    }
    sendVideoUpdate(video.paused ? "Pause" : "Play");
}

document.addEventListener("keydown", e => {
    switch (e.key.toLocaleLowerCase()) {
        case " ":
            togglePlay();
            break;
        case "m":
            toggleMute();
            break;
        case "arrowleft":
            skip(-10);
            break;
        case "arrowright":
            skip(10);
            break;
    }
})

// function for popup redirect
function confirmpopup(id) {
    let message = {
        "type": "SwitchTo",
        "id": id
    };
    ws.send(JSON.stringify(message));
}

function reload() {
    let paused = video.paused;
    video.pause();
    video.currentTime = 0;

    tmp = video.src;
    video.src = "";
    video.src = tmp;

    let popup = document.querySelector(".popup");
    popup.parentNode.removeChild(popup);// TODO: Make this failing not matter

    if (!paused) {
        video.play();
    }
}
