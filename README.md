
on 2 terminals:
cargo run 
cargo run

Test Chat:
Bob: /chat Hello everyone
Alice sees: Chat [Bob]: Hello everyone
Alice: /chat Hi Bob
Bob sees: Chat [Alice]: Hi Bob

Test DM:
Bob: /dm Alice Hi There
Alice sees: DM from Bob: Hi There
Bob sees: Direct message delivered.
Alice: /dm Bob Hello
Bob sees: DM from Alice: Hello

Test File Sharing (if files exist):
Bob: /getfile Alice notes.txt ./local_notes.txt
(Assuming Alice has notes.txt, it should save to Bob’s local_notes.txt.)

Test List Peers:
Bob: /listpeers
Alice sees: Peers: [Bob]

Alice: /listpeers
Bob sees: Peers: [Alice]
