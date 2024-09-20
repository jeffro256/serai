TODO

  async fn publish_completion(
    &self,
    completion: &<Self::Eventuality as EventualityTrait>::Completion,
  ) -> Result<(), NetworkError> {
    // Publish this to the dedicated TX server for a solver to actually publish
    #[cfg(not(test))]
    {
    }

    // Publish this using a dummy account we fund with magic RPC commands
    #[cfg(test)]
    {
      let router = self.router().await;
      let router = router.as_ref().unwrap();

      let mut tx = match completion.command() {
        RouterCommand::UpdateSeraiKey { key, .. } => {
          router.update_serai_key(key, completion.signature())
        }
        RouterCommand::Execute { outs, .. } => router.execute(
          &outs.iter().cloned().map(Into::into).collect::<Vec<_>>(),
          completion.signature(),
        ),
      };
      tx.gas_limit = 1_000_000u64.into();
      tx.gas_price = 1_000_000_000u64.into();
      let tx = ethereum_serai::crypto::deterministically_sign(&tx);

      if self.provider.get_transaction_by_hash(*tx.hash()).await.unwrap().is_none() {
        self
          .provider
          .raw_request::<_, ()>(
            "anvil_setBalance".into(),
            [
              tx.recover_signer().unwrap().to_string(),
              (U256::from(tx.tx().gas_limit) * U256::from(tx.tx().gas_price)).to_string(),
            ],
          )
          .await
          .unwrap();

        let (tx, sig, _) = tx.into_parts();
        let mut bytes = vec![];
        tx.encode_with_signature_fields(&sig, &mut bytes);
        let pending_tx = self.provider.send_raw_transaction(&bytes).await.unwrap();
        self.mine_block().await;
        assert!(pending_tx.get_receipt().await.unwrap().status());
      }

      Ok(())
    }
  }

  #[cfg(test)]
  async fn get_transaction_by_eventuality(
    &self,
    block: usize,
    eventuality: &Self::Eventuality,
  ) -> Self::Transaction {
    // We mine 96 blocks to ensure the 32 blocks relevant are finalized
    // Back-check the prior two epochs in response to this
    // TODO: Review why this is sub(3) and not sub(2)
    for block in block.saturating_sub(3) ..= block {
      match eventuality.1 {
        RouterCommand::UpdateSeraiKey { nonce, .. } | RouterCommand::Execute { nonce, .. } => {
          let router = self.router().await;
          let router = router.as_ref().unwrap();

          let block = u64::try_from(block).unwrap();
          let filter = router
            .key_updated_filter()
            .from_block(block * 32)
            .to_block(((block + 1) * 32) - 1)
            .topic1(nonce);
          let logs = self.provider.get_logs(&filter).await.unwrap();
          if let Some(log) = logs.first() {
            return self
              .provider
              .get_transaction_by_hash(log.clone().transaction_hash.unwrap())
              .await
              .unwrap()
              .unwrap();
          };

          let filter = router
            .executed_filter()
            .from_block(block * 32)
            .to_block(((block + 1) * 32) - 1)
            .topic1(nonce);
          let logs = self.provider.get_logs(&filter).await.unwrap();
          if logs.is_empty() {
            continue;
          }
          return self
            .provider
            .get_transaction_by_hash(logs[0].transaction_hash.unwrap())
            .await
            .unwrap()
            .unwrap();
        }
      }
    }
    panic!("couldn't find completion in any three of checked blocks");
  }

  #[cfg(test)]
  async fn mine_block(&self) {
    self.provider.raw_request::<_, ()>("anvil_mine".into(), [96]).await.unwrap();
  }

  #[cfg(test)]
  async fn test_send(&self, send_to: Self::Address) -> Self::Block {
    use rand_core::OsRng;
    use ciphersuite::group::ff::Field;
    use ethereum_serai::alloy::sol_types::SolCall;

    let key = <Secp256k1 as Ciphersuite>::F::random(&mut OsRng);
    let address = ethereum_serai::crypto::address(&(Secp256k1::generator() * key));

    // Set a 1.1 ETH balance
    self
      .provider
      .raw_request::<_, ()>(
        "anvil_setBalance".into(),
        [Address(address).to_string(), "1100000000000000000".into()],
      )
      .await
      .unwrap();

    let value = U256::from_str_radix("1000000000000000000", 10).unwrap();
    let tx = ethereum_serai::alloy::consensus::TxLegacy {
      chain_id: None,
      nonce: 0,
      gas_price: 1_000_000_000u128,
      gas_limit: 200_000u128,
      to: ethereum_serai::alloy::primitives::TxKind::Call(send_to.0.into()),
      // 1 ETH
      value,
      input: ethereum_serai::router::abi::inInstructionCall::new((
        [0; 20].into(),
        value,
        vec![].into(),
      ))
      .abi_encode()
      .into(),
    };

    use ethereum_serai::alloy::{primitives::Signature, consensus::SignableTransaction};
    let sig = k256::ecdsa::SigningKey::from(k256::elliptic_curve::NonZeroScalar::new(key).unwrap())
      .sign_prehash_recoverable(tx.signature_hash().as_ref())
      .unwrap();

    let mut bytes = vec![];
    tx.encode_with_signature_fields(&Signature::from(sig), &mut bytes);
    let pending_tx = self.provider.send_raw_transaction(&bytes).await.ok().unwrap();

    // Mine an epoch containing this TX
    self.mine_block().await;
    assert!(pending_tx.get_receipt().await.unwrap().status());
    // Yield the freshly mined block
    self.get_block(self.get_latest_block_number().await.unwrap()).await.unwrap()
  }
