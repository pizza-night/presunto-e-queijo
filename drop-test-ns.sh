#!/bin/bash

clients=(peq1 peq2 peq3)

for c in ${clients[@]}; do
    sudo ip link del veth-$c-host
    sudo ip netns del $c
done

sudo ip link del br0
