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

Run ```keydisp```. Now the ```index.html``` should display all keyboard input. To use with OBS add ```index.html``` as a browser source to OBS.