# Installation

You need the nightly rust compiler to run the server, you can get the rust installation tool ```rustup``` at [rustup.rs](https://rustup.rs/)


To install nightly run
```
rustup toolchain install nightly
```

When the nightly toolchain is installed you can install keydisp with
```
cd server\keydisp
cargo install
```

Run ```keydisp```. Press F10 to select the current foreground window. Now the navigate to ```index.html```, it should display keyboard input from the selected window. To use with OBS add ```index.html``` as a browser source to OBS.
