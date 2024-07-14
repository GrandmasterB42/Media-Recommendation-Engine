<h1 align="center">Media Recommendation Engine</h1>

Media Recommendation Engine(name is subject to change) aims to be a (optionally)self-hosted media manager and recommendation site. It is currently in a very early stage of development and not ready for use.

## Current capabilities

- Detect video files in a directory structure(to be documented), organize them in a database
- Show the discovered content in a web interface
- Basic multi user support
- Basic settings for adding storage locations and users
- Shared media playback sessions

## Short term goals

- Improve and implement more basic features like watch history/progress, permissions,...
- Many more settings and much more diagnostic information in the web interface
- Preview thumbnails and timelines for videos
- Data Manipulation from the web interface
- Much more data presented in the web interface (this information will have to be entered manually for now)
- Implement recommendation that is not just getting the next video
- Documentation/Testing in CI for the project (mainly directory structure and building the project)

## Long term/aspirational goals

- Have cross-media recommendation
- Transcoding of media files | Possibly upscaling
- A System where you can add friends and servers will grab metadata from each other to fill out their own databases
- Integration with some third party services
- Setup a "public" database that can hold all this information, so individual users don't have to manually organize their content, or even possibly make a non-self-hosted version of this project
- Native apps, maybe using something like tauri and also a chrome extension

## Notes on Chrome Extension

The Chrome Extension is basically non-existent at the moment, just me trying out stuff.
To build the Extension, execute "build_ext.cmd". The finished Extension will be located in the newly created pkg directory in the root directory.

This only works on windows. If anyone has an idea for a cross-platform solution or can easily port it to a .sh or similar, a pull request would be appreciated. Maybe I will transition that to a python script

## License

I don't have any experience with licensing, so for now I will not include one. If you have any insight into which licenses I would need to respect or what I must consider for future additions like adding ffmpeg as a dependency, please let me know.

## Contribution

If you want to contribute, open an issue and I will set up a discord server for further discussion. I do not expect anyone to notice this project as I am not advertising it anywhere, but who knows, maybe this will become usable at some point and people would want to use it.
