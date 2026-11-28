use crate::{Value, eval};

#[test]
fn list_vs_index_syntax() {
    let src = r#"
        (let (xs [10 20 30])
          (+ xs[0] xs[(+ 1 1)]))
    "#;
    assert_eq!(eval(src).unwrap(), Value::Int(40));
}

#[test]
fn higher_order_fold_and_filter() {
    let src = r#"
        (let (evens (filter (fn (x) (== (% x 2) 0)) (range 0 6)))
          (fold (fn (acc x) (+ acc x)) 0 evens))
    "#;
    assert_eq!(eval(src).unwrap(), Value::Int(6));
}

#[test]
fn lisp_list_operations() {
    assert_eq!(eval("(is_empty [])").unwrap(), Value::Bool(true));
    assert_eq!(eval("(cons 1 [2 3])").unwrap(), eval("[1 2 3]").unwrap());
    assert_eq!(eval("(push [1 2] 3)").unwrap(), eval("[1 2 3]").unwrap());
    assert_eq!(
        eval("(append [1] [2 3] [4])").unwrap(),
        eval("[1 2 3 4]").unwrap()
    );
    assert_eq!(eval("(car [10 20])").unwrap(), eval("some(10)").unwrap());
    assert_eq!(eval("(car [])").unwrap(), Value::None);
    assert_eq!(eval("(cdr [10 20 30])").unwrap(), eval("[20 30]").unwrap());
    assert_eq!(eval("(cdr [])").unwrap(), eval("[]").unwrap());
    assert_eq!(eval("(last [10 20])").unwrap(), eval("some(20)").unwrap());
    assert_eq!(eval("(get [10 20] 5)").unwrap(), Value::None);
    assert_eq!(eval("(reverse [1 2 3])").unwrap(), eval("[3 2 1]").unwrap());
    assert_eq!(eval("(take [1 2 3 4] 2)").unwrap(), eval("[1 2]").unwrap());
    assert_eq!(eval("(drop [1 2 3 4] 2)").unwrap(), eval("[3 4]").unwrap());
    assert_eq!(
        eval("(slice [10 20 30 40] 1 3)").unwrap(),
        eval("[20 30]").unwrap()
    );
    assert_eq!(
        eval("(flatten [[1 2] [3]])").unwrap(),
        eval("[1 2 3]").unwrap()
    );
    assert_eq!(
        eval("(flat_map (fn (x) [x (* x 10)]) [1 2])").unwrap(),
        eval("[1 10 2 20]").unwrap()
    );
    assert_eq!(eval("(contains [1 2 3] 2)").unwrap(), Value::Bool(true));
    assert_eq!(
        eval("(find (fn (x) (> x 3)) [1 4 5])").unwrap(),
        eval("some(4)").unwrap()
    );
    assert_eq!(
        eval("(any (fn (x) (> x 3)) [1 4])").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        eval("(all (fn (x) (> x 0)) [1 2])").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        eval("(zip [1 2] [\"a\" \"b\" \"c\"])").unwrap(),
        eval("[(tuple 1 \"a\") (tuple 2 \"b\")]").unwrap()
    );
    assert_eq!(
        eval("(enumerate [\"a\" \"b\"])").unwrap(),
        eval("[(tuple 0 \"a\") (tuple 1 \"b\")]").unwrap()
    );
    assert_eq!(eval("(sort [3 1 2])").unwrap(), eval("[1 2 3]").unwrap());
    assert_eq!(
        eval("(sort_by (fn (x) (- 0 x)) [1 3 2])").unwrap(),
        eval("[3 2 1]").unwrap()
    );
    assert_eq!(
        eval("(unique [1 2 1 3 2])").unwrap(),
        eval("[1 2 3]").unwrap()
    );
    assert_eq!(eval("(list 1 2 3)").unwrap(), eval("[1 2 3]").unwrap());
    let rec_map = r#"
        (defn my_map ((f (fn (i64) i64)) (xs (list i64))) -> (list i64)
          (match (car xs)
            (none [])
            (some(h) (cons (f h) (my_map f (cdr xs))))))
        (my_map (fn (x) (* x 2)) (list 1 2 3))
    "#;
    assert_eq!(eval(rec_map).unwrap(), eval("(list 2 4 6)").unwrap());
}

#[test]
fn list_array_conversion_and_indexing() {
    let src = r#"
        (let (arr (list->array (list 10 20 30)))
          (+ arr[0] (len arr) (match (get arr 2) (some(v) v) (none 0))))
    "#;
    assert_eq!(eval(src).unwrap(), Value::Int(43));
    assert_eq!(
        eval("(array->list (list->array [1 2 3]))").unwrap(),
        eval("(list 1 2 3)").unwrap()
    );
}
