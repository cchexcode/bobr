# bobr

`bobr` is a simple command multiplexer.

## How to use

`bobr` will execute all given commands in parallel and give an overview of the status these are in. It is useful for executing multiple commands in parallel, whether it is to speed things up or to run multiple long running commands in parallel (like starting backend HTTP server and frontend).

- `bobr -c "sleep 5" -c "sleep 10" -c "sleep 2 && exit 1"`
- `bobr -c "sleep 5" -f ./tasks.sh`

Note: Duplicate commands will automatically be deduplicated (for now).
