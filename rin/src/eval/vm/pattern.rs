use super::*;

impl Runner<'_, '_> {
    pub(in crate::eval) fn match_pattern(&self, pat: &Pattern, val: RVal) -> Option<Option<RVal>> {
        match (pat, val) {
            (Pattern::Wildcard, _) => Some(None),
            (Pattern::Var(_), v) => Some(Some(v)),
            (Pattern::Int(a), RVal::Int(b)) if *a == b => Some(None),
            (Pattern::Bool(a), RVal::Bool(b)) if *a == b => Some(None),
            (Pattern::Char(a), RVal::Char(b)) if *a == b => Some(None),
            (Pattern::Str(a), RVal::Obj(id)) => match self.pool.get(id) {
                Obj::Str(s) if a.as_str() == s.as_str() => Some(None),
                _ => None,
            },
            (Pattern::Ident(a), RVal::Obj(id)) => match self.pool.get(id) {
                Obj::Ident(s) if a.as_str() == s.as_str() => Some(None),
                _ => None,
            },
            (Pattern::None, RVal::None) => Some(None),
            (Pattern::Some(bind), RVal::Obj(id)) => match self.pool.get(id) {
                Obj::Some(inner) => Some(bind.as_ref().map(|_| *inner)),
                _ => None,
            },
            (Pattern::Ok(bind), RVal::Obj(id)) => match self.pool.get(id) {
                Obj::Ok(inner) => Some(bind.as_ref().map(|_| *inner)),
                _ => None,
            },
            (Pattern::Err(bind), RVal::Obj(id)) => match self.pool.get(id) {
                Obj::Err(inner) => Some(bind.as_ref().map(|_| *inner)),
                _ => None,
            },
            (
                Pattern::Variant {
                    type_name,
                    variant,
                    binding,
                },
                RVal::Obj(id),
            ) => match self.pool.get(id) {
                Obj::Variant {
                    type_name: vt,
                    variant: vv,
                    data,
                } if type_name.as_str() == vt.as_str() && variant.as_str() == vv.as_str() => {
                    match (binding, data) {
                        (Some(_), Some(d)) => Some(Some(*d)),
                        (None, _) => Some(None),
                        (Some(_), None) => None,
                    }
                }
                _ => None,
            },
            _ => None,
        }
    }
}
