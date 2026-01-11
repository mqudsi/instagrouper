# INSTAGROUPER(1)

## NAME
instagrouper — (video) media asset recombination and metadata generation utility

## SYNOPSIS
**instagrouper** [**-o** *outdir* | **--out-dir** *outdir*] *file* ...
**instagrouper** [**-h** | **--help**]

## DESCRIPTION
**instagrouper** is a command-line utility designed to reconstruct original media attachments from fragmented assets typically found on Content Delivery Networks (CDNs) and social media platforms. Given a set of $N$ input files generated from $M$ original media files - consisting of various combinations of isolated audio streams, video streams in varying resolutions, and static screenshots of the same - the utility identifies files belonging to the same source material and recombines them into $M$ unified attachments.

The utility performs the following operations:

1. **Identification**: Utilizes **ffprobe(1)** to extract codecs, durations, resolutions, and temporal metadata.
2. **Grouping**: Correlates disparate files into logical groups based on duration (within a certain threshold), timestamps, and content type.
3. **Optimization**: Within each group, selects the highest resolution video stream available and the most suitable audio stream.
4. **Recombination**: Invokes **ffmpeg(1)** to perform a fast stream-copy (remux, not re-encode) of the identified assets into a standardized MP4 container, with zero generational quality loss for maximal speed and quality.
5. **Thumbnail Generation**: Creates a visual preview for each reconstructed attachment, including a playback overlay for video content and an "audio-only" thumbnail for generated assets containing only audio stream(s).
6. **Metadata Export**: Generates a comprehensive JSON representation of the resulting attachments, including file paths, timestamps, sizes, and original source mappings.

## OPTIONS
**-o**, **--out-dir** *directory*
        Specify the directory where merged media and thumbnails will be written. Defaults to the current working directory. The directory must exist prior to execution.

**-h**, **--help**
        Display usage information and exit.

## DEPENDENCIES
The following external binaries must be present in the system's PATH:

*   **ffmpeg**: Required for the muxing of audio/video streams and thumbnail generation.
*   **ffprobe**: Required for analyzing input file metadata and stream configurations.

**instagrouper** has been tested on **ffmpeg** versions 6 and above.

## IMPLEMENTATION
**instagrouper** is developed in Rust. It utilizes the following internal logic:

*  **Parallel Processing**: Media processing is distributed across available CPU cores using a thread pool.
*  **Temporal Analysis**: Attempts to extract timestamps from the individual streams, containers, and files provided, and use that time and date information to aid in the logical grouping process.
*  **Source Mapping**: The JSON output (on *stdout*) of the utility provides a one-to-many mapping between each generated media file and the source assets (individual audio, video, or image inputs) that are semantically equivalent to the same.
*  **Media Passthrough**: Extra input image assets not found to belong to any of the recombined audio/video streams are passed through as additional media files.

## OUTPUT

On a successful run, **instagrouper**'s *stdout* is guaranteed to be valid, standards-conforming JavaScript (presently, always in human-readable format) containing information about the results of the processing job. Additional debug data is emitted to *stderr* in realtime and does not affect the processing of JSON output on *stdout*. Two files are created for each recombined media asset: the remuxed MP4 container with the merged audio and video (where available), and a thumbnail.

## COMPATIBILITY AND FFMPEG VERSIONS
The utility currently employs **ffmpeg** filter syntax compatible with version 6.0 and earlier (specifically the `scale2ref` filter). While this remains functional on **ffmpeg** version 7.0 and later, it now triggers deprecation warnings. The source code internally supports both legacy and newer versions of **ffmpeg**.

Future iterations of **instagrouper** will include either a command line option to specify the version of **ffmpeg** on the system, or else dynamically detect the installed **ffmpeg** version and adjust the filtergraph syntax accordingly (e.g., utilizing the `split` and `scale` reference syntax introduced in **ffmpeg** version 7) to guarantee compatibility.

**instagrouper** is compatible with all major operating systems where the rust toolchain and FFmpeg are supported, including FreeBSD, Linux, macOS, and Windows.

## EXIT STATUS
The **instagrouper** utility exits 0 on success, and >0 if an error occurs (e.g., missing dependencies, invalid output directory, corrupt input media, or ffmpeg process failure).

## EXAMPLES

### Scenario 1: Basic separate audio and video streams
An input directory contains `source_1080.mp4` (video only) and `source_audio.mp4` (audio only).

```bash
$ instagrouper -o ./output source_1080.mp4 source_audio.mp4
[
  {
    "name": "source_000.mp4",
    "path": "/absolute/path/output/source_000.mp4",
    "timestamp": "2024-05-20T12:00:00Z",
    "size": 5242880,
    "size_pretty": "5.00 MiB",
    "kind": "audio+video",
    "thumbnail": "/absolute/path/output/source_000.jpg",
    "duration": "00:45",
    "sources": [
      "/absolute/path/source_1080.mp4",
      "/absolute/path/source_audio.mp4"
    ]
  }
]
```

### Scenario 2: Handling multiple resolutions
If provided with 360p, 720p, and 1080p versions of the same content, **instagrouper** will group them and select the 1080p stream for the final output.

```bash
$ instagrouper asset_360.mp4 asset_1080.mp4 asset_720.mp4
Merged 3 files into 1 attachments
...
```

### Scenario 3: Mixed media assets
If provided with individual streams from multiple original sources, provided as-is with no grouping or labeling, **instagrouper** will attempt to determine which streams belong to which inputs and group them accordingly.

```bash
$ instagrouper asset1_audio.mp4 asset1_360.mp4 asset1_1080.mp4 asset1.jpg \
      asset2_audio.mp4 asset2_360.mp4 asset2_1080.mp4 \
      asset3_720.mp4 asset3.jpg \
      asset4.jpg
Merged 10 files into 4 attachments
...
```

## AUTHOR
Mahmoud Al-Qudsi <mqudsi@neosmart.net>

## LICENSE
This project is licensed under the terms of the MIT License.
