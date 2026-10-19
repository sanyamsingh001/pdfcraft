use hayro_syntax::object::array::{ArrayIter, FlexArrayIter};
use hayro_syntax::object::dict::keys::{BASE_STATE, D, OCGS, OCMD, OCPROPERTIES, OFF, ON, P, TYPE};
use hayro_syntax::object::{Array, Dict, Name, Object, ObjectIdentifier};
use std::collections::HashSet;

pub(crate) struct OcgState {
    inactive_ocgs: HashSet<ObjectIdentifier>,
    visibility_stack: Vec<bool>,
}

impl OcgState {
    fn dummy() -> Self {
        Self {
            inactive_ocgs: HashSet::default(),
            visibility_stack: vec![],
        }
    }

    pub(crate) fn from_catalog(catalog: &Dict<'_>) -> Self {
        let Some(oc_properties) = catalog.get::<Dict<'_>>(OCPROPERTIES) else {
            return Self::dummy();
        };

        let Some(config) = oc_properties.get::<Dict<'_>>(D) else {
            return Self::dummy();
        };

        let mut inactive = HashSet::new();

        let base_state = config
            .get::<Name<'_>>(BASE_STATE)
            .and_then(|b| BaseState::from_name(b.as_ref()));

        if base_state.unwrap_or(BaseState::On) == BaseState::Off
            && let Some(ocgs) = oc_properties.get::<Array<'_>>(OCGS)
        {
            for item in ocgs.raw_iter() {
                if let Some(ref_) = item.as_obj_ref() {
                    let id: ObjectIdentifier = ref_.into();
                    inactive.insert(id);
                }
            }
        }

        let mut read_ocg_array = |key, insert_active: bool| {
            if let Some(arr) = config.get::<Array<'_>>(key) {
                for item in arr.raw_iter() {
                    if let Some(ref_) = item.as_obj_ref() {
                        let id: ObjectIdentifier = ref_.into();
                        if insert_active {
                            inactive.remove(&id);
                        } else {
                            inactive.insert(id);
                        }
                    }
                }
            }
        };

        read_ocg_array(ON, true);
        read_ocg_array(OFF, false);

        Self {
            inactive_ocgs: inactive,
            visibility_stack: Vec::new(),
        }
    }

    pub(crate) fn begin_single_oc(&mut self, ocg_id: ObjectIdentifier) {
        let is_active = !self.inactive_ocgs.contains(&ocg_id);
        let visible = self.is_visible() && is_active;
        self.visibility_stack.push(visible);
    }

    pub(crate) fn begin_ocmd(&mut self, ocmd: &Dict<'_>) {
        // ISO 32000-1, 8.11.2.2: a supported /VE takes precedence over /OCGs and /P.
        // Unsupported expressions use the entire existing policy, never a partial result.
        if let Some(expression) = ocmd.get::<Array<'_>>(b"VE") {
            if let Some(active) = self.expression_visible(&expression) {
                self.visibility_stack.push(self.is_visible() && active);
                return;
            }
            warn!("unsupported optional-content visibility expression; using membership policy");
        }
        let policy = ocmd
            .get::<Name<'_>>(P)
            .and_then(|n| OcmdPolicy::from_name(n.as_ref()))
            .unwrap_or(OcmdPolicy::AnyOn);

        let mut ocg_ids: Vec<ObjectIdentifier> = Vec::new();

        if let Some(arr) = ocmd.get::<Array<'_>>(OCGS) {
            for item in arr.raw_iter() {
                if let Some(ref_) = item.as_obj_ref() {
                    ocg_ids.push(ref_.into());
                }
            }
        } else if let Some(ref_) = ocmd.get_ref(OCGS) {
            ocg_ids.push(ref_.into());
        }

        let is_active = if ocg_ids.is_empty() {
            true
        } else {
            match policy {
                OcmdPolicy::AllOn => ocg_ids.iter().all(|id| !self.inactive_ocgs.contains(id)),
                OcmdPolicy::AnyOn => ocg_ids.iter().any(|id| !self.inactive_ocgs.contains(id)),
                OcmdPolicy::AnyOff => ocg_ids.iter().any(|id| self.inactive_ocgs.contains(id)),
                OcmdPolicy::AllOff => ocg_ids.iter().all(|id| self.inactive_ocgs.contains(id)),
            }
        };

        let visible = self.is_visible() && is_active;
        self.visibility_stack.push(visible);
    }

    fn expression_visible(&self, expression: &Array<'_>) -> Option<bool> {
        // Finite work per expression, including repeated references. No Rust recursion and
        // no visibility-state mutation until the complete expression has been validated.
        const MAX_DEPTH: usize = 64;
        const MAX_VISITS: usize = 4096;
        let mut visits = 1usize;
        let mut stack = vec![ExpressionFrame::new(expression)?];
        loop {
            let frame = stack.last_mut()?;
            if let Some(raw) = frame.raw.next() {
                visits = visits.checked_add(1)?;
                if visits > MAX_VISITS {
                    return None;
                }
                // A resolved iterator alone can terminate at an unresolved operand. Match
                // every raw item so that such a prefix is never accepted as the expression.
                let object = frame.resolved.next::<Object<'_>>()?;
                match object {
                    Object::Array(child) => {
                        if stack.len() >= MAX_DEPTH {
                            return None;
                        }
                        visits = visits.checked_add(1)?; // The nested operator also counts.
                        if visits > MAX_VISITS {
                            return None;
                        }
                        stack.push(ExpressionFrame::new(&child)?);
                    }
                    Object::Dict(group) => {
                        let reference = raw.as_obj_ref()?;
                        if group.get::<Name<'_>>(TYPE).as_deref() != Some(b"OCG") {
                            return None;
                        }
                        frame.accept(!self.inactive_ocgs.contains(&reference.into()))?;
                    }
                    _ => return None,
                }
            } else {
                let value = frame.finish()?;
                stack.pop();
                if let Some(parent) = stack.last_mut() {
                    parent.accept(value)?;
                } else {
                    return Some(value);
                }
            }
        }
    }

    pub(crate) fn begin_ocg(&mut self, props: &Dict<'_>, ref_id: ObjectIdentifier) {
        match props.get::<Name<'_>>(TYPE).as_deref() {
            Some(OCMD) => self.begin_ocmd(props),
            _ => self.begin_single_oc(ref_id),
        }
    }

    pub(crate) fn begin_marked_content(&mut self) {
        let visible = self.is_visible();
        self.visibility_stack.push(visible);
    }

    pub(crate) fn end_marked_content(&mut self) {
        self.visibility_stack.pop();
    }

    /// PdfCraft patch: override one group's state (viewer layer toggles).
    pub(crate) fn set_active(&mut self, id: ObjectIdentifier, active: bool) {
        if active {
            self.inactive_ocgs.remove(&id);
        } else {
            self.inactive_ocgs.insert(id);
        }
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visibility_stack.last().copied().unwrap_or(true)
    }
}

// Keep both iterators: raw entries preserve OCG identity and distinguish exhaustion
// from failed resolution. And/Or consume every operand even after the value is known.
struct ExpressionFrame<'a> {
    raw: ArrayIter<'a>,
    resolved: FlexArrayIter<'a>,
    operator: ExpressionOperator,
    operands: usize,
    value: bool,
}

#[derive(Clone, Copy)]
enum ExpressionOperator {
    Not,
    And,
    Or,
}

impl<'a> ExpressionFrame<'a> {
    fn new(array: &Array<'a>) -> Option<Self> {
        let mut raw = array.raw_iter();
        raw.next()?;
        let mut resolved = array.flex_iter();
        let name = resolved.next::<Name<'_>>()?;
        let operator = match name.as_ref() {
            b"Not" => ExpressionOperator::Not,
            b"And" => ExpressionOperator::And,
            b"Or" => ExpressionOperator::Or,
            _ => return None,
        };
        Some(Self {
            raw,
            resolved,
            operator,
            operands: 0,
            value: matches!(operator, ExpressionOperator::And),
        })
    }

    fn accept(&mut self, value: bool) -> Option<()> {
        self.operands = self.operands.checked_add(1)?;
        self.value = match self.operator {
            ExpressionOperator::Not if self.operands == 1 => !value,
            ExpressionOperator::Not => return None,
            ExpressionOperator::And => self.value && value,
            ExpressionOperator::Or => self.value || value,
        };
        Some(())
    }

    fn finish(&self) -> Option<bool> {
        (self.operands > 0).then_some(self.value)
    }
}

impl Default for OcgState {
    fn default() -> Self {
        Self::dummy()
    }
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
enum BaseState {
    On,
    Off,
    Unchanged,
}

impl BaseState {
    fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"ON" => Some(Self::On),
            b"OFF" => Some(Self::Off),
            b"Unchanged" => Some(Self::Unchanged),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
enum OcmdPolicy {
    AllOn,
    AnyOn,
    AnyOff,
    AllOff,
}

impl OcmdPolicy {
    fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"AllOn" => Some(Self::AllOn),
            b"AnyOn" => Some(Self::AnyOn),
            b"AnyOff" => Some(Self::AnyOff),
            b"AllOff" => Some(Self::AllOff),
            _ => None,
        }
    }
}
