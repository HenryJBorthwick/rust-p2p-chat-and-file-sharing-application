COSC 473 – Decentralised Applications on the Web
Assignment 2 - SwapBytes

**Introduction**
The goal of this first assignment is to create a P2P file bartering application called SwapBytes using libp2p. In the app a user will be able to offer a file to swap for another file, and other users
on other peers will be able to accept the swap. For example, you might be willing to share yourMachine Learning class notes for someone else's class notes from last week's COSC 473 tutorial. :) This assignment will help you demonstrate understanding of the basic concepts of libp2p and P2P networking, including peer discovery, establishing connections, data exchange, and file transfer.

**Basic requirements**
Implement a simple user interface (CLI) for users to send and receive messages and
files.

Peers have a user defined nickname and other users should be able to identify them by that in the UI (e.g., for direct messaging or file sharing).

A pubsub chat room for users to propose the file they want, and what files they are
willing to share.

Peers that have connected for a trade in the chat room should be able to send direct messages to each other.

Once peers have agreed to swap files they should be able to send the files to each other using a request/response pattern.

You will need a mechanism to bootstrap your network, so peers can discover each other. You can use mDNS, but there should be some way to potentially connect to another peer not on the same local area network. You might use a rendezvous server for peer discovery for that. However, you do not need to implement NAT traversal (hole punching).
