> require!(price.conf < price.price / MAX_CONF_RATIO, ProtocolError::OracleConfidenceBreached);
>