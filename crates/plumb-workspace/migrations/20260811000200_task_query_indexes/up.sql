CREATE INDEX tasks_state_wait
    ON tasks(closure_state, wait_millis, path, start);
