# Design: epoch binding as a precondition

Every execution path calls one helper before invoking runtime start/restore/fork. The helper first issues a nonzero monotonic epoch, then durably binds it to the instance row. Either error aborts execution. The database update must affect exactly one row; an absent instance is not a successful bind. Runtime work therefore cannot become ready without a journaled epoch that grant validation can observe.
