use std::collections::HashSet;

use cel::common::ast::operators;
use cel::common::ast::{Expr, IdedExpr, LiteralValue};

use crate::TaskWorkflowState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TaskPredicateField {
    Id,
    Title,
    Created,
    Due,
    Priority,
    Wait,
    Done,
    Canceled,
    Recur,
    Prev,
}

impl TaskPredicateField {
    fn from_ident(name: &str) -> Option<Self> {
        Some(match name {
            "id" => Self::Id,
            "title" => Self::Title,
            "created" => Self::Created,
            "due" => Self::Due,
            "priority" => Self::Priority,
            "wait" => Self::Wait,
            "done" => Self::Done,
            "canceled" => Self::Canceled,
            "recur" => Self::Recur,
            "prev" => Self::Prev,
            _ => return None,
        })
    }

    fn kind(self) -> TaskPredicateFieldKind {
        match self {
            Self::Id | Self::Title | Self::Recur | Self::Prev => TaskPredicateFieldKind::String,
            Self::Priority => TaskPredicateFieldKind::Integer,
            Self::Created | Self::Due | Self::Wait | Self::Done | Self::Canceled => {
                TaskPredicateFieldKind::Timestamp
            }
        }
    }

    fn nullable(self) -> bool {
        self != Self::Title
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskPredicateFieldKind {
    String,
    Integer,
    Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskPredicateOp {
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

impl TaskPredicateOp {
    fn reversed(self) -> Self {
        match self {
            Self::Equal | Self::NotEqual => self,
            Self::Less => Self::Greater,
            Self::LessEqual => Self::GreaterEqual,
            Self::Greater => Self::Less,
            Self::GreaterEqual => Self::LessEqual,
        }
    }

    fn is_equality(self) -> bool {
        matches!(self, Self::Equal | Self::NotEqual)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskPredicateValue {
    Null,
    String(String),
    Integer(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskCandidatePredicate {
    Constant(bool),
    And(Vec<Self>),
    Or(Vec<Self>),
    Not(Box<Self>),
    Compare {
        field: TaskPredicateField,
        op: TaskPredicateOp,
        value: TaskPredicateValue,
    },
    State(TaskWorkflowState),
}

impl TaskCandidatePredicate {
    pub(crate) fn and(predicates: impl IntoIterator<Item = Self>) -> Option<Self> {
        combine(predicates, true)
    }

    pub(crate) fn or(predicates: impl IntoIterator<Item = Self>) -> Option<Self> {
        combine(predicates, false)
    }
}

fn combine(
    predicates: impl IntoIterator<Item = TaskCandidatePredicate>,
    conjunction: bool,
) -> Option<TaskCandidatePredicate> {
    let mut combined = Vec::new();
    for predicate in predicates {
        match predicate {
            TaskCandidatePredicate::Constant(value) if value != conjunction => {
                return Some(TaskCandidatePredicate::Constant(value));
            }
            TaskCandidatePredicate::Constant(_) => {}
            TaskCandidatePredicate::And(nested) if conjunction => combined.extend(nested),
            TaskCandidatePredicate::Or(nested) if !conjunction => combined.extend(nested),
            predicate => combined.push(predicate),
        }
    }
    match combined.len() {
        0 => Some(TaskCandidatePredicate::Constant(conjunction)),
        1 => combined.pop(),
        _ if conjunction => Some(TaskCandidatePredicate::And(combined)),
        _ => Some(TaskCandidatePredicate::Or(combined)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskCandidatePrefix {
    pub(crate) predicate: Option<TaskCandidatePredicate>,
    pub(crate) complete: bool,
}

pub(crate) fn task_candidate_prefix(expression: &IdedExpr) -> TaskCandidatePrefix {
    let mut conjuncts = Vec::new();
    flatten_conjunction(expression, &mut conjuncts);
    let mut predicates = Vec::new();
    let mut non_null = HashSet::new();
    let mut complete = true;
    for conjunct in conjuncts {
        let Some(predicate) = translate_total(conjunct, &non_null) else {
            complete = false;
            break;
        };
        if let Some(field) = non_null_when_true(conjunct) {
            non_null.insert(field);
        }
        predicates.push(predicate);
    }
    TaskCandidatePrefix {
        predicate: TaskCandidatePredicate::and(predicates),
        complete,
    }
}

fn flatten_conjunction<'a>(expression: &'a IdedExpr, output: &mut Vec<&'a IdedExpr>) {
    if let Some((left, right)) = binary_call(expression, operators::LOGICAL_AND) {
        flatten_conjunction(left, output);
        flatten_conjunction(right, output);
    } else {
        output.push(expression);
    }
}

fn translate_total(
    expression: &IdedExpr,
    non_null: &HashSet<TaskPredicateField>,
) -> Option<TaskCandidatePredicate> {
    match &expression.expr {
        Expr::Literal(LiteralValue::Boolean(value)) => {
            Some(TaskCandidatePredicate::Constant(value.into_inner()))
        }
        Expr::Call(call) if call.func_name == operators::LOGICAL_NOT && call.args.len() == 1 => {
            translate_total(&call.args[0], non_null)
                .map(|predicate| TaskCandidatePredicate::Not(Box::new(predicate)))
        }
        Expr::Call(call) if call.func_name == operators::LOGICAL_OR && call.args.len() == 2 => {
            TaskCandidatePredicate::or([
                translate_total(&call.args[0], non_null)?,
                translate_total(&call.args[1], non_null)?,
            ])
        }
        Expr::Call(call) if call.args.len() == 2 => {
            let op = comparison_op(&call.func_name)?;
            translate_comparison(&call.args[0], op, &call.args[1], non_null)
        }
        _ => None,
    }
}

fn translate_comparison(
    left: &IdedExpr,
    op: TaskPredicateOp,
    right: &IdedExpr,
    non_null: &HashSet<TaskPredicateField>,
) -> Option<TaskCandidatePredicate> {
    if let Some(field) = ident(left) {
        return comparison_with_field(field, op, literal(right)?, non_null);
    }
    if let Some(field) = ident(right) {
        return comparison_with_field(field, op.reversed(), literal(left)?, non_null);
    }
    None
}

fn comparison_with_field(
    name: &str,
    op: TaskPredicateOp,
    value: TaskPredicateValue,
    non_null: &HashSet<TaskPredicateField>,
) -> Option<TaskCandidatePredicate> {
    if name == "state" {
        if !op.is_equality() {
            return None;
        }
        let TaskPredicateValue::String(value) = value else {
            return None;
        };
        let state = match value.as_str() {
            "waiting" => TaskWorkflowState::Waiting,
            "done" => TaskWorkflowState::Done,
            "canceled" => TaskWorkflowState::Canceled,
            "conflicted" => TaskWorkflowState::Conflicted,
            _ => return None,
        };
        let predicate = TaskCandidatePredicate::State(state);
        return Some(if op == TaskPredicateOp::NotEqual {
            TaskCandidatePredicate::Not(Box::new(predicate))
        } else {
            predicate
        });
    }

    let field = TaskPredicateField::from_ident(name)?;
    let compatible = match (&value, field.kind()) {
        (TaskPredicateValue::Null, _) => field.nullable() && op.is_equality(),
        (TaskPredicateValue::String(_), TaskPredicateFieldKind::String) => op.is_equality(),
        (TaskPredicateValue::Integer(_), TaskPredicateFieldKind::Integer) => {
            op.is_equality() || !field.nullable() || non_null.contains(&field)
        }
        _ => false,
    };
    compatible.then_some(TaskCandidatePredicate::Compare { field, op, value })
}

fn non_null_when_true(expression: &IdedExpr) -> Option<TaskPredicateField> {
    let Expr::Call(call) = &expression.expr else {
        return None;
    };
    let op = comparison_op(&call.func_name)?;
    if call.args.len() != 2 {
        return None;
    }
    let (field, value) = if let Some(field) = ident(&call.args[0]) {
        (
            TaskPredicateField::from_ident(field)?,
            literal(&call.args[1])?,
        )
    } else {
        (
            TaskPredicateField::from_ident(ident(&call.args[1])?)?,
            literal(&call.args[0])?,
        )
    };
    match (op, value) {
        (TaskPredicateOp::NotEqual, TaskPredicateValue::Null) => Some(field),
        (TaskPredicateOp::Equal, TaskPredicateValue::String(_))
        | (TaskPredicateOp::Equal, TaskPredicateValue::Integer(_)) => Some(field),
        _ => None,
    }
}

fn comparison_op(name: &str) -> Option<TaskPredicateOp> {
    Some(match name {
        operators::EQUALS => TaskPredicateOp::Equal,
        operators::NOT_EQUALS => TaskPredicateOp::NotEqual,
        operators::LESS => TaskPredicateOp::Less,
        operators::LESS_EQUALS => TaskPredicateOp::LessEqual,
        operators::GREATER => TaskPredicateOp::Greater,
        operators::GREATER_EQUALS => TaskPredicateOp::GreaterEqual,
        _ => return None,
    })
}

fn binary_call<'a>(expression: &'a IdedExpr, name: &str) -> Option<(&'a IdedExpr, &'a IdedExpr)> {
    let Expr::Call(call) = &expression.expr else {
        return None;
    };
    (call.func_name == name && call.args.len() == 2).then(|| (&call.args[0], &call.args[1]))
}

fn ident(expression: &IdedExpr) -> Option<&str> {
    let Expr::Ident(name) = &expression.expr else {
        return None;
    };
    Some(name)
}

fn literal(expression: &IdedExpr) -> Option<TaskPredicateValue> {
    let Expr::Literal(literal) = &expression.expr else {
        return None;
    };
    Some(match literal {
        LiteralValue::Null => TaskPredicateValue::Null,
        LiteralValue::String(value) => TaskPredicateValue::String(value.inner().to_string()),
        LiteralValue::Int(value) => TaskPredicateValue::Integer(*value.inner()),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use cel::Program;

    use super::*;

    fn prefix(source: &str) -> TaskCandidatePrefix {
        let program = Program::compile(source).unwrap();
        task_candidate_prefix(program.expression())
    }

    #[test]
    fn extracts_total_leading_conjuncts_and_stops_at_the_first_residual() {
        let planned =
            prefix("state == 'done' && priority != null && priority > 2 && size(depends_on) > 0");
        assert!(!planned.complete);
        assert!(matches!(
            planned.predicate,
            Some(TaskCandidatePredicate::And(ref predicates)) if predicates.len() == 3
        ));
    }

    #[test]
    fn rejects_unguarded_nullable_ordering_without_hiding_later_predicates() {
        let planned = prefix("priority > 2 && state == 'done'");
        assert!(!planned.complete);
        assert_eq!(
            planned.predicate,
            Some(TaskCandidatePredicate::Constant(true))
        );
    }

    #[test]
    fn translates_exact_or_only_when_both_branches_are_supported() {
        assert!(prefix("state == 'done' || title == 'Done'").complete);
        assert!(!prefix("state == 'done' || title.startsWith('D')").complete);
    }

    #[test]
    fn translates_null_checks_and_non_dependency_workflow_states() {
        assert!(prefix("due == null && state == 'waiting'").complete);
        assert!(prefix("id != null && id == 'task'").complete);
        assert!(prefix("priority != null && priority < -2").complete);
        assert!(!prefix("state == 'ready'").complete);
        assert!(!prefix("state == 'blocked'").complete);
    }
}
