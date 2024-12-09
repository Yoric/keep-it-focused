# About

keep-it-focused is a small tool designed to help users focus on their task by making sure
that some applications or websites are permitted only during specific days/times.

As of this writing, this application works only on Linux + Firefox. If you wish to expand
it to other platforms, patches are welcome!

# Setting up

## Dependencies

1. The Rust toolchain
    If Rust does not come with your distro, see the instructions at https://rustup.rs.
2. npm
    If npm does not come with your distro, see the instructions at https://docs.npmjs.com/downloading-and-installing-node-js-and-npm .
3. GNU make
    If make does not come with your distro, you probably have bigger issues.
4. An account on https://addons.mozilla.org
    You'll need this to be able to build and install new versions of the Firefox addon. Sorry, we don't make the rules!

## Credentials

(only needed to build the webextension for release)

Get your API keys for https://addons.mozilla.org at https://addons.mozilla.org/en-US/developers/addon/api/key/ .

Write them to a file called `.env` in this directory:

```sh
AMO_API_KEY=(your JWT issuer key)
AMO_API_SECRET=(your JWT secret)
```


## Building

```sh
# Install other dependencies.
$ make init

# Rebuild from source code (suprisingly, the addon is the slowest part to build).
$ make all
```

## Installing/reinstalling

The following command will:

1. copy the addon to a system-wide repository;
2. force all Firefox profiles on the machine (current and future) to install the addon;
3. copy the binary as a system binary;
4. setup the binary as a system daemon, which will restart automatically upon the next startup;
5. start the daemon immediately;
6. create an empty configuration file at /etc/keep-it-focused.yaml.

```sh
$ sudo target/release/keep-it-focused setup
```

or

```sh
$ sudo make install
```

Don't hesitate to look at the help for more info on running only some of these steps:

```sh
$ target/release/keep-it-focused help setup
```

# Using on a daily basis

As of this writing, keep-it-focused has the following features:

1. You can setup a configuration file that specifies _when_ (combination of day and time intervals) you're allowed to use some binaries/websites. Any binary/website that is not mentioned is permitted.
    - If you attempt to open a binary that you shouldn't be opening during a given time slot, the binary will close after ~1 minute.
    - If you're running a binary and the permitted slot is almost finished, you'll receive a warning, giving you a little time to save your game^H^H^H^H^H work, then the binary will close.
    - If you attempt to navigate to a website you shouldn't be opening during a given time slot, the website won't load.
    - If you're looking at a website and the permitted slot is almost finished, you'll receive a warning, giving you a little time to save your game^H^H^H^H^H work, then the tab will empty itself.
    - Note: Website blocking features require the companion add-on.
2. You can add temporary authorizations that are only valid until the end of the day.

## Permanent configuration

The file containing your permanent configuration is at `/etc/keep-it-focused.yaml`. See `resources/test.yaml` for an example.

You can also use the command-line to add new authorization intervals, see e.g.

```sh
$ keep-it-focused permanently --help
```

You must be `root` (or `sudo`) to make these changes.

If you make changes manually, we recommand you use

```sh
$ cargo run -- check your_file.yaml
```

before overwriting `/etc/keep-it-focused.yaml`!

## Temporary authorizations

You can add any number of files that are valid only for the current day in `/tmp/keep-it-focused`.

You can also use the command-line to add new temporary authorization intervals, see e.g.

```sh
$ keep-it-focused exceptionally --help
```

