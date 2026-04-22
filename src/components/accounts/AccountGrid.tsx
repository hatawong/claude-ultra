import type { Account } from '../../types/account';
import AccountCard from './AccountCard';

interface AccountGridProps {
  accounts: Account[];
}

function AccountGrid({ accounts }: AccountGridProps) {
  return (
    <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
      {accounts.map((account) => (
        <AccountCard key={account.accountId} account={account} />
      ))}
    </div>
  );
}

export default AccountGrid;
