use futures::stream::StreamExt;
use scylla::{
    host_filter::AllowListHostFilter, prepared_statement::PreparedStatement,
    statement::Consistency, transport::errors::QueryError, QueryResult, Session, SessionBuilder,
};
use std::{
    io::Write,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
    time::Instant,
};

async fn create_session(node_addr: &str) -> Arc<Session> {
    let session: Session = SessionBuilder::new()
        .known_node(node_addr)
        .default_consistency(Consistency::Quorum)
        .build()
        .await
        .unwrap();

    Arc::new(session)
}

// Create a Session that opens connections only to this particular node.
async fn create_single_node_session(node_addr: &str) -> Arc<Session> {
    let session: Session = SessionBuilder::new()
        .known_node(node_addr)
        .default_consistency(Consistency::One)
        .host_filter(Arc::new(
            AllowListHostFilter::new(std::iter::once(node_addr)).unwrap(),
        ))
        .build()
        .await
        .unwrap();

    Arc::new(session)
}

async fn setup_keyspace_table_and_view(session: &Session) {
    session
        .query("DROP KEYSPACE IF EXISTS view_test", ())
        .await
        .unwrap();

    session
        .query(
            "CREATE KEYSPACE view_test with replication = \
             {'class': 'SimpleStrategy', 'replication_factor': 3}",
            (),
        )
        .await
        .unwrap();

    session
        .query(
            "CREATE TABLE view_test.tab (p int, c int, r int, primary key (p, c))",
            (),
        )
        .await
        .unwrap();

    session
        .query(
            "CREATE MATERIALIZED VIEW view_test.tab_view AS SELECT p, c, r FROM view_test.tab \
             WHERE p IS NOT NULL and c IS NOT NULL PRIMARY KEY ((p, c))",
            (),
        )
        .await
        .unwrap();
}

struct WritesStopper {
    should_stop: Arc<AtomicBool>,
}

impl WritesStopper {
    pub fn new(should_stop: Arc<AtomicBool>) -> WritesStopper {
        WritesStopper { should_stop }
    }

    pub fn stop(&self) {
        self.should_stop.store(true, Ordering::Relaxed);
    }
}

// Starts <concurrency> fibers which perform an insert every <frequency_interval> seconds.
// The writes will stop after calling WritesStopper::stop().
async fn start_writes(session: &Arc<Session>) -> WritesStopper {
    let concurrency = 512;
    let frequency_interval = Duration::from_millis(100);

    let should_stop: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let report_gap = Duration::from_secs(4);

    let mut insert: PreparedStatement = session
        .prepare("INSERT INTO view_test.tab (p, c, r) VALUES (?, ?, ?)")
        .await
        .unwrap();
    insert.set_is_idempotent(true);
    let insert = Arc::new(insert);

    // Start <concurrency> fibers which perform an insert every <frequency_interval> seconds.
    for id in 0..concurrency {
        let session = session.clone();
        let insert = insert.clone();

        let should_stop = should_stop.clone();

        tokio::spawn(async move {
            // Print the first report after 1 second.
            let mut last_report = Instant::now() - report_gap + Duration::from_secs(1);

            let mut interval = tokio::time::interval(frequency_interval);
            for i in 0.. {
                // Send a query every <frequency_interval> seconds.
                interval.tick().await;

                // Stop the writes when told to.
                if should_stop.load(Ordering::Relaxed) {
                    return;
                }

                // The first fiber reports the total number of requests sent every ~<report_gap> seconds.
                if id == 0 && last_report.elapsed() > report_gap {
                    println!(
                        "Wrote ~{} rows... Press Enter to stop the writes and verify.",
                        i * concurrency
                    );
                    last_report = std::time::Instant::now();
                }

                // Perform the insert
                let value = id + i * concurrency;
                let tries_num = 8;
                for try_number in 1.. {
                    let result: Result<QueryResult, QueryError> =
                        session.execute(&insert, (value, value, value)).await;

                    match &result {
                        Ok(_) => break,
                        Err(_) if try_number < tries_num => {
                            tokio::time::sleep(Duration::from_millis(64)).await;
                        }
                        Err(_) => {
                            let _ = result.unwrap();
                        }
                    }
                }
            }
        });
    }

    WritesStopper::new(should_stop)
}

async fn read_all_rows_from_table(session: &Session, table: &str) -> Vec<(i32, i32, i32)> {
    let mut select: PreparedStatement = session
        .prepare(format!("SELECT p, c, r FROM view_test.{table}"))
        .await
        .unwrap();
    select.set_is_idempotent(true);

    let mut rows: Vec<(i32, i32, i32)> = Vec::new();

    let mut rows_iter = session
        .execute_iter(select, ())
        .await
        .unwrap()
        .into_typed::<(i32, i32, i32)>();
    while let Some(row) = rows_iter.next().await {
        rows.push(row.unwrap());
    }
    rows.sort();

    rows
}

fn print_msg(message: &str) {
    println!("{}", message);
    std::io::stdout().flush().unwrap();
}

#[tokio::main]
async fn main() {
    // The test expects a 5 node cluster with the following node addresses:
    let nodes = [
        "127.0.0.1:9042",
        "127.0.0.2:9042",
        "127.0.0.3:9042",
        "127.0.0.4:9042",
        "127.0.0.5:9042",
    ];

    print_msg("Connecting...");

    // Create the main session - connects to all nodes, uses CL_QUORUM
    let session: Arc<Session> = create_session(nodes[0]).await;

    // Create sessions to each of the nodes - they connect to this node only,
    // the default consistency is CL_ONE.
    let node_sessions: &[Arc<Session>] = &[
        create_single_node_session(nodes[0]).await,
        create_single_node_session(nodes[1]).await,
        create_single_node_session(nodes[2]).await,
        create_single_node_session(nodes[3]).await,
        create_single_node_session(nodes[4]).await,
    ];

    setup_keyspace_table_and_view(&session).await;

    print_msg(
        "Starting the writes, please kill and restart one node while the writes are being sent.",
    );
    let writes_stopper: WritesStopper = start_writes(&session).await;

    std::io::stdin().read_line(&mut String::new()).unwrap();

    writes_stopper.stop();
    print_msg("Stopped the writes, let's wait 60s for the cluster to settle.");

    tokio::time::sleep(Duration::from_secs(60)).await;

    loop {
        print_msg("Verifying view table integrity...");
        // Read all rows form the base table using CL=QUORUM
        let base_rows: Vec<(i32, i32, i32)> = read_all_rows_from_table(&session, "tab").await;

        for (node_id, single_node_session) in node_sessions.iter().enumerate() {
            // Read all view rows from this node using CL=ONE
            let view_rows: Vec<(i32, i32, i32)> =
                read_all_rows_from_table(&single_node_session, "tab_view").await;

            if view_rows == base_rows {
                print_msg(&format!("View from node #{node_id} matches the base table"));
            } else {
                print_msg(&format!(
                    "ERROR: View from node #{node_id} doesn't match the base table"
                ));
                print_msg(&format!(
                    "base_rows.len(): {}, view_rows.len(): {}",
                    base_rows.len(),
                    view_rows.len(),
                ));
            }
        }

        print_msg("\nThe check will be repeated after 60s");
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}
