# mvandcrash
A test to see if materialized views stay true to the base table after a node goes down in the cluster.

## Description
This tests runs on a 5 node cluster.\

It sets up the environment:
```cql
CREATE KEYSPACE view_test with replication = {'class': 'SimpleStrategy', 'replication_factor': 3};
CREATE TABLE view_test.tab (p int, c int, r int, primary key (p, c));
CREATE MATERIALIZED VIEW view_test.tab_view AS SELECT p, c, r FROM view_test.tab WHERE p IS NOT NULL and c IS NOT NULL PRIMARY KEY ((p, c))
```

Then it starts writing to the table.
During the writes one node should be manually killed and then brought back up.
Then the writes are stopped and the runner verifies if the rows in the base table match the ones in the view table.

To acquire the base table rows the runner selects all rows with consistency `QUORUM`.\
Then, for each node, it selects all rows from the view table with consistency `ONE` and checks if the rows match the base table rows.

The verification is repeated every 60 seconds, Scylla is eventually consistent, achieving full consistency could take some time.

## Requirements:
This test requires [scylla-ccm](https://github.com/scylladb/scylla-ccm) and [`Rust`](https://www.rust-lang.org/tools/install)

## How to run the test

Use `ccm` to setup a test cluster with 5 nodes, their ips should be `127.0.0.1`, `127.0.0.2`, `127.0.0.3`, `127.0.0.4`, `127.0.0.5`:
```bash
$ ccm create mycluster -n 5 --scylla --vnodes -v release:2022.2.2
$ ccm start 
$ ccm status
Cluster: 'mycluster'
--------------------
node1: UP
node2: UP
node3: UP
node4: UP
node5: UP
```

Start the test:
```
$ cargo run --release
Running `target/release/mvandcrash`
Connecting...
Starting the writes, please kill and restart one node while the writes are being sent.
Wrote ~5120 rows... Press Enter to stop the writes and verify.
Wrote ~26112 rows... Press Enter to stop the writes and verify.
Wrote ~47104 rows... Press Enter to stop the writes and verify.
...
```

Find the PID of a node to kill:
```bash
$ ps aux | grep mycluster
user   3748877  3.2  1.3 17180028528 428564 ?  Sl   18:30   0:17 /home/user/.ccm/mycluster/node3/bin/scylla --options-file ...
...
```

Kill the node:
```bash
$ kill -9 3748877
```

Wait a moment, then bring the node up again:
```bash
$ ccm start node3
```

Let the cluster recover, then press `Enter` to stop the writes
The test will wait for the cluster to settle and then check if the view table matches the base table.
Eventually the tables should contain the same rows.

```
Stopped the writes, let's wait 60s for the cluster to settle.
Verifying view table integrity...
```

This output indicates that everything is as it should be:
```
View from node #0 matches the base table
View from node #1 matches the base table
View from node #2 matches the base table
View from node #3 matches the base table
View from node #4 matches the base table

The check will be repeated after 60s
```

This output indicates an inconsistency between the view table and the base table:
```
View from node #0 matches the base table
View from node #1 matches the base table
ERROR: View from node #2 doesn't match the base table
base_rows.len(): 222068, view_rows.len(): 222066
View from node #3 matches the base table
View from node #4 matches the base table

The check will be repeated after 60s
```

## Example output
Here's example output of a run with errors: [example-output.txt](example-output.txt)
