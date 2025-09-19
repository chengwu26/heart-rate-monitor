### Heart Rate Monitor

This tool reads heart rate data from HRS (Heart Rate Service) compatible
devices (e.g., smart bands) and launches an HTTP service, allowing you to
display real-time heart rate metrics in OBS Studio for live streaming or
recordings.

![Default UI](doc/default_ui.png)

### Features

- Connects to compatible heart rate monitoring devices (MiBand 10 and similar)
- Provides real-time heart rate data via HTTP endpoint
- Simple integration with OBS Studio using Browser Source
- Customizable heart rate UI by replacing `heart_rate.html` file
- Cross-platform support (Windows, macOS, Linux)

### How It Works

1. The tool connects to your smart band via Bluetooth LE
2. It continuously receives heart rate broadcasts from the device
3. An embedded HTTP server provides the data at http://127.0.0.1:3030/heart-rate
4. OBS Studio can display the data using a Browser Source

### Acknowledgments

This project was inspired by and builds upon the work of
[miband-heart-rate](https://github.com/Tnze/miband-heart-rate).
Special thanks to the original author for their Bluetooth LE implementation.

### Important Notice

This project is currently in active development.

Please note:
- Functionality may be incomplete or unstable
- APIs are subject to change
- Documentation may not be fully up-to-date

We welcome contributions and feedback as we continue to improve this tool!
