#!/bin/bash

clients=(peq1 peq2 peq3)

bash -x ./drop-test-ns.sh
ip --color=always addr
sleep 1

for c in ${clients[@]}; do
    sudo ip netns add $c
done

sudo ip link add br0 type bridge
sudo ip link set dev br0 up

ci=1
for c in ${clients[@]}; do
    sudo ip link add veth-$c type veth peer name veth-$c-host
    sudo ip link set veth-$c netns $c
    sudo ip link set veth-$c-host master br0
    sudo ip link set veth-$c-host up
    sudo ip netns exec $c ip link set veth-$c up
    sudo ip netns exec $c ip addr add 10.1.1.$((ci++))/24 dev veth-$c
    #sudo ip addr add 10.1.1.$((++ci))/24 dev veth-$c-host
    # sudo ip netns exec $c ip route add default via 10.1.1.$ci
done

sudo ip addr add 10.1.1.254/24 dev br0

ip --color=always addr
