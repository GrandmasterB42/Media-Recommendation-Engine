let ws;
let isUserEvent = true;
let justJoined = true;

// Workaround for https://github.com/bigskysoftware/htmx/issues/2183 / https://github.com/bigskysoftware/htmx/issues/764 to fix picture-in-picture
let temp_video = document.getElementById("currentvideo");
temp_video.replaceWith(temp_video.cloneNode(true));
// ---

const video = document.getElementById("currentvideo");

const videocontainer = document.querySelector(".video-container");

document.body.addEventListener("htmx:wsOpen", function (event) {
    ws = event.detail.socketWrapper;
});

document.body.addEventListener("htmx:wsBeforeMessage", function (event) {
    try {
        var data = JSON.parse(event.detail.message);
        handleServerEvent(data)
    } catch (e) {
        // This fails if html is sent, so it doesnt really matter
    }
});

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
    let volumeLevel
    if (video.muted || video.volume === 0) {
        volumeslider.value = 0;
        volumeLevel = "muted";
    } else if (video.volume < 0.5) {
        volumeLevel = "low";
    } else {
        volumeLevel = "high";
    }

    videocontainer.dataset.volumeLevel = volumeLevel
});

// Modes
function toggleFullscreenMode() {
    if (document.fullscreenElement == null) {
        videocontainer.requestFullscreen();
    } else {
        document.exitFullscreen();
    }
}

document.addEventListener("fullscreenchange", function () {
    videocontainer.classList.toggle("full-screen", document.fullscreenElement);
})

function togglePiPMode() {
    if (videocontainer.classList.contains("pip")) {
        document.exitPictureInPicture();
    } else {
        try {
            video.requestPictureInPicture();
        } catch (e) {
            console.log(e);
        }
    }
}

video.addEventListener("enterpictureinpicture", function () {
    if (!videocontainer.classList.contains("pip")) { videocontainer.classList.add("pip"); }
})

video.addEventListener("leavepictureinpicture", function () {
    videocontainer.classList.remove("pip");
})

// Play / Pause
function togglePlay() {
    video.paused ? video.play() : video.pause();
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

video.addEventListener("play", () => {
    videocontainer.classList.remove("paused");
    if (isUserEvent) {
        sendVideoState("Play", video.currentTime);
    }
    isUserEvent = false;
})

video.addEventListener("pause", () => {
    videocontainer.classList.add("paused");
    if (isUserEvent) {
        sendVideoState("Pause", video.currentTime);
    }
    isUserEvent = false
})
// ---

video.addEventListener("seeked", () => {
    if (!justJoined) {
        if (isUserEvent) {
            sendVideoState("Seek", video.currentTime)
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