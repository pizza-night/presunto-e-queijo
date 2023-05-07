#!/bin/bash


total_clients=0
batch_size=1
while :; do
    for _ in $(seq 1 $batch_size); do
        netcat 10.1.1.1 2504 < /dev/random >/dev/null &
        ((++total_clients))
    done
    ((++batch_size))

    echo -e "\n$total_clients netcats running"
    sleep 10
done
