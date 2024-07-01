<h1 align="center">Media Recommendation Engine</h1>

Media Recommendation Engine(name is subject to change) aims to be a self-hosted media manager and recommendation site. It is currently in a very early stage of development and not ready for use.

## Current capabilities

- Detect video files in a plex-like directory structure, organize them in a database
- Show the discovered content in a web interface
- Shared media playback sessions

## Short term goals

- Extract as much information as possible from local files
- Improve and implement more basic features like authentication, multiple user accounts, watch history/progress, ...
- Actually recommend content and not just list it

## Long term/aspirational goals

- Support for multiple media types like music, books, games, ... that are all organized for recommendation
- Transcoding of media files | Possibly upscaling
- A System where you can add friends and servers will grab metadata from each other to fill out their own databases
- Integration with some third party services
- Setup a "public" database that can hold all this information, so individual users don't have to manually organize their content, or even possibly make a non-self-hosted version of this project
- Native apps, maybe using something like tauri

## Notes on Chrome Extension

The Chrome Extension is even more experimental than the rest of this project.
To build the Extension, execute "build_ext.cmd". The finished Extension will be located in the newly created pkg directory in the root directory.

This only works on windows. If anyone has an idea for a cross-platform solution or can easily port it to a .sh or similar, a pull request would be appreciated.

## License

I don't have any experience with licensing, so for now I will not include one. If you have any insight into which licenses I would need to respect or what I must consider for future additions like adding ffmpeg as a dependency, please let me know.

## Contribution

If you want to contribute, open an issue and I will set up a discord server for further discussion. I do not expect anyone to notice this project as I am not advertising it anywhere, but who knows, maybe this will become usable at some point and people would want to use it.
