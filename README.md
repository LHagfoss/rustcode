# rustcode

<img src="./image.png" alt="rustcode screenshot" />

## initial
`fmr` now `rustcode` is a lightweight Terminal User Interface (TUI) harness for testing Apple's on-device Foundation Models via `fm serve`. 

## new
Now supports ollama or openai compatible APIs.
Major UI overhaul taking ALOT of inspo from OpenCodes Agent Harness.

## Running the Application

1. Start the Apple Foundation Models completions server:
    ```bash
    fm serve
    ```
2. Build and run the TUI:
   `bash
    cargo run
    `
   OR you can install it via `cargo install` and run it from anywhere:
   `bash
    cargo install --path .
    rustcode
    `

## IMPORTANT!

If you wanna run `rustcode` using Apple FM you NEED to be on MacOS 27 and have XCode v27 for this to work. As this was introduced in the Beta version of MacOS 27.
