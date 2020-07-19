[![CircleCI](https://circleci.com/bb/iptvdream/web-settings.svg?style=svg)](https://circleci.com/bb/iptvdream/web-settings)

Server which provides web interface for editing settings in the Set-Top-Box or
some IoT devices.
This repository has only the server side implementation.
Program is written in [rust](https://www.rust-lang.org/) using [actix-web](https://actix.rs/) framework.

## Usage
The device sends list of config parameters definitions to the server, 
currently we can have string, integer, bool and choice box 
(see [example](https://bitbucket.org/iptvdream/web-settings/src/master/example.json)).

```bash
curl 'http://localhost:8000/stb/new-session' -X POST -H "Content-Type: application/json" -d @example.json -s
```

```json
{"key":"qrsT1w","secret":"AtxW3kwOIeXFty0q-WAoopnYISL-zMSWz8zAapGovoirSBSwCpuvBiVjFFYs6CSuHlG6YOSmv66MjrCercfdOg"}
```

The resulting `key` is displayed to the user, with which he can access web interface.
The device starts polling server for changes using `secret`.
We have http polling for simplicity.
The devices also tells the server the revision of the settings values it currently has.
It is incremented each time the user submits changes.

```bash
curl 'http://localhost:8000/stb/poll?sid=AtxW3kwOIeXFty0q-WAoopnYISL-zMSWz8zAapGovoirSBSwCpuvBiVjFFYs6CSuHlG6YOSmv66MjrCercfdOg&revision=0' -s
```

When changes were submitted, the server replies with incremented revision 
and the setting specification which contains new values.

```json
{
  "revision":1,
  "values":[
    {
      "name":"a",
      "title":"TestA",
      "type":"string",
      "value":"new text"
    },
    {
      "name":"b",
      "title":"TestB",
      "type":"integer",
      "min":0,
      "max":100,
      "value":100
    },
    {
      "name":"c",
      "title":"TestC",
      "type":"selection",
      "value":"bar",
      "options":[
        {
          "value":"foo",
          "title":"Foo!"
        },
        {
          "value":"bar",
          "title":"Bar!"
        }
      ]
    },
    {
      "name":"d",
      "title":"TestD",
      "type":"bool",
      "value":false
    }
  ]
}
```


## Compilation
Basically it is just `cargo build --release`.
You can examine my [circleci config](https://bitbucket.org/iptvdream/web-settings/src/master/.circleci/config.yml) to get more insight into the required build commands. 

## Example of systemd unit
Create a new service file under the `/etc/systemd/system` with the following content
```systemd
[Unit]
Description=Web settings server
After=network.target

[Service]
User=your_username
Environment=RUST_LOG=info
Environment=RUST_BACKTRACE=1
WorkingDirectory=/opt/web-settings
ExecStart=/opt/web-settings/app --port 3080
StandardOutput=syslog
StandardError=syslog
SyslogIdentifier=web-settings

[Install]
WantedBy=multi-user.target
```
This will start the server at localhost:3080, afterwards you can set up nginx
reverse proxy to get the host and https done right.


## Note to developer
- Keep code clean by using `cargo clippy`
- Keep dependencies updated with `cargo update`
